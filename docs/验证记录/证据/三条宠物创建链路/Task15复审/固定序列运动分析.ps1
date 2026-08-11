param(
    [Parameter(Mandatory = $true)]
    [string] $FramesDir,

    [Parameter(Mandatory = $true)]
    [string] $ProfilePath,

    [Parameter(Mandatory = $true)]
    [string] $ReferencePath,

    [Parameter(Mandatory = $true)]
    [string] $OutputPath
)

$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing.dll -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

public sealed class FixedSequenceFrameMetric
{
    public string Frame { get; set; }
    public string ComparisonFrame { get; set; }
    public double Dx { get; set; }
    public double Dy { get; set; }
    public double RotationDegrees { get; set; }
    public double FaceBoundaryResidual { get; set; }
    public double BreathBoundaryResidual { get; set; }
    public double FaceLumaResidual { get; set; }
    public double BreathLumaResidual { get; set; }
}

public static class FixedSequenceMotionAnalyzer
{
    private const int ForegroundThreshold = 18;
    // 39 frames = three 5.2 s sway periods and 4/7 of a 2.8 s breath period.
    // That keeps sway phase fixed while giving the closest sampled offset to opposed breath.
    private const int SameSwayOpposedBreathStride = 39;

    private sealed class Raster
    {
        public int Width;
        public int Height;
        public bool[] Foreground;
        public bool[] Edge;
        public byte[] Luma;
    }

    private struct Region
    {
        public double Left;
        public double Top;
        public double Right;
        public double Bottom;

        public bool Contains(double x, double y)
        {
            return x >= Left && x < Right && y >= Top && y < Bottom;
        }
    }

    private sealed class ActualFrameResiduals
    {
        public double FaceBoundary;
        public double BreathBoundary;
        public double FaceLuma;
        public double BreathLuma;
    }

    public static FixedSequenceFrameMetric[] Analyze(
        string[] paths,
        string referencePath,
        double faceLeft,
        double faceTop,
        double faceRight,
        double faceBottom,
        double breathLeft,
        double breathTop,
        double breathRight,
        double breathBottom,
        double pivotX,
        double pivotY)
    {
        if (paths == null || paths.Length < 2) throw new ArgumentException("at least two frames are required");
        Raster reference = Load(referencePath);
        Region face = new Region { Left = faceLeft, Top = faceTop, Right = faceRight, Bottom = faceBottom };
        Region breath = new Region { Left = breathLeft, Top = breathTop, Right = breathRight, Bottom = breathBottom };
        List<Point> referenceFaceEdges = EdgePoints(reference, face);
        if (referenceFaceEdges.Count < 20) throw new InvalidOperationException("faceSafety alpha contour is too small for rigid fitting");

        var metrics = new List<FixedSequenceFrameMetric>(paths.Length);
        for (int index = 0; index < paths.Length; index++)
        {
            Raster current = Load(paths[index]);
            int comparisonIndex = index + SameSwayOpposedBreathStride < paths.Length
                ? index + SameSwayOpposedBreathStride
                : index - SameSwayOpposedBreathStride;
            Raster comparison = Load(paths[comparisonIndex]);
            if (current.Width != reference.Width || current.Height != reference.Height)
                throw new InvalidOperationException("all frames must have the same dimensions");
            if (comparison.Width != reference.Width || comparison.Height != reference.Height)
                throw new InvalidOperationException("all comparison frames must have the same dimensions");

            double bestDx = 0;
            double bestDy = 0;
            double bestRotation = 0;
            FitRigid(referenceFaceEdges, current, pivotX, pivotY, out bestDx, out bestDy, out bestRotation);
            double comparisonDx = 0;
            double comparisonDy = 0;
            double comparisonRotation = 0;
            FitRigid(
                referenceFaceEdges,
                comparison,
                pivotX,
                pivotY,
                out comparisonDx,
                out comparisonDy,
                out comparisonRotation);
            ActualFrameResiduals residuals = CompareActualFrames(comparison, current,
                face, breath, pivotX, pivotY,
                comparisonDx, comparisonDy, comparisonRotation,
                bestDx, bestDy, bestRotation);

            metrics.Add(new FixedSequenceFrameMetric {
                Frame = System.IO.Path.GetFileName(paths[index]),
                ComparisonFrame = System.IO.Path.GetFileName(paths[comparisonIndex]),
                Dx = bestDx,
                Dy = bestDy,
                RotationDegrees = bestRotation,
                FaceBoundaryResidual = residuals.FaceBoundary,
                BreathBoundaryResidual = residuals.BreathBoundary,
                FaceLumaResidual = residuals.FaceLuma,
                BreathLumaResidual = residuals.BreathLuma,
            });
        }
        return metrics.ToArray();
    }

    private static Raster Load(string path)
    {
        using (var source = new Bitmap(path))
        using (var bitmap = new Bitmap(source.Width, source.Height, PixelFormat.Format24bppRgb))
        {
            using (Graphics graphics = Graphics.FromImage(bitmap))
            {
                graphics.Clear(Color.White);
                graphics.DrawImageUnscaled(source, 0, 0);
            }
            var rectangle = new Rectangle(0, 0, bitmap.Width, bitmap.Height);
            BitmapData data = bitmap.LockBits(rectangle, ImageLockMode.ReadOnly, PixelFormat.Format24bppRgb);
            try
            {
                int stride = Math.Abs(data.Stride);
                byte[] raw = new byte[stride * bitmap.Height];
                Marshal.Copy(data.Scan0, raw, 0, raw.Length);
                var raster = new Raster {
                    Width = bitmap.Width,
                    Height = bitmap.Height,
                    Foreground = new bool[bitmap.Width * bitmap.Height],
                    Edge = new bool[bitmap.Width * bitmap.Height],
                    Luma = new byte[bitmap.Width * bitmap.Height],
                };
                for (int y = 0; y < bitmap.Height; y++)
                {
                    int sourceRow = data.Stride >= 0 ? y * stride : (bitmap.Height - 1 - y) * stride;
                    for (int x = 0; x < bitmap.Width; x++)
                    {
                        int sourceIndex = sourceRow + x * 3;
                        byte blue = raw[sourceIndex];
                        byte green = raw[sourceIndex + 1];
                        byte red = raw[sourceIndex + 2];
                        int targetIndex = y * bitmap.Width + x;
                        raster.Foreground[targetIndex] = Math.Max(255 - red, Math.Max(255 - green, 255 - blue)) > ForegroundThreshold;
                        raster.Luma[targetIndex] = (byte)((77 * red + 150 * green + 29 * blue) >> 8);
                    }
                }
                for (int y = 1; y < bitmap.Height - 1; y++)
                {
                    for (int x = 1; x < bitmap.Width - 1; x++)
                    {
                        int i = y * bitmap.Width + x;
                        raster.Edge[i] = raster.Foreground[i] && (
                            !raster.Foreground[i - 1] || !raster.Foreground[i + 1]
                            || !raster.Foreground[i - bitmap.Width] || !raster.Foreground[i + bitmap.Width]);
                    }
                }
                return raster;
            }
            finally
            {
                bitmap.UnlockBits(data);
            }
        }
    }

    private static List<Point> EdgePoints(Raster raster, Region region)
    {
        var points = new List<Point>();
        for (int y = Math.Max(1, (int)Math.Floor(region.Top)); y < Math.Min(raster.Height - 1, (int)Math.Ceiling(region.Bottom)); y++)
            for (int x = Math.Max(1, (int)Math.Floor(region.Left)); x < Math.Min(raster.Width - 1, (int)Math.Ceiling(region.Right)); x++)
                if (raster.Edge[y * raster.Width + x]) points.Add(new Point(x, y));
        return points;
    }

    private static void FitRigid(
        List<Point> referenceEdges,
        Raster current,
        double pivotX,
        double pivotY,
        out double bestDx,
        out double bestDy,
        out double bestRotation)
    {
        int bestMatches = -1;
        double bestMotionMagnitude = double.PositiveInfinity;
        bestDx = 0;
        bestDy = 0;
        bestRotation = 0;
        for (int rotationStep = -8; rotationStep <= 8; rotationStep++)
        {
            double rotation = rotationStep / 10.0;
            double radians = rotation * Math.PI / 180.0;
            double cosine = Math.Cos(radians);
            double sine = Math.Sin(radians);
            for (int dy = -4; dy <= 4; dy++)
            {
                for (int dx = -4; dx <= 4; dx++)
                {
                    int matches = 0;
                    for (int pointIndex = 0; pointIndex < referenceEdges.Count; pointIndex += 2)
                    {
                        Point point = referenceEdges[pointIndex];
                        double tx;
                        double ty;
                        Transform(point.X, point.Y, pivotX, pivotY, dx, dy, cosine, sine, out tx, out ty);
                        if (HasEdge(current, (int)Math.Round(tx), (int)Math.Round(ty), 1)) matches++;
                    }
                    double motionMagnitude = Math.Abs(dx) + Math.Abs(dy) + Math.Abs(rotation);
                    if (matches > bestMatches || (matches == bestMatches && motionMagnitude < bestMotionMagnitude))
                    {
                        bestMatches = matches;
                        bestMotionMagnitude = motionMagnitude;
                        bestDx = dx;
                        bestDy = dy;
                        bestRotation = rotation;
                    }
                }
            }
        }
    }

    private static ActualFrameResiduals CompareActualFrames(
        Raster comparison,
        Raster current,
        Region face,
        Region breath,
        double pivotX,
        double pivotY,
        double comparisonDx,
        double comparisonDy,
        double comparisonRotation,
        double currentDx,
        double currentDy,
        double currentRotation)
    {
        return new ActualFrameResiduals {
            FaceBoundary = BoundaryResidual(comparison, current, face, pivotX, pivotY,
                comparisonDx, comparisonDy, comparisonRotation, currentDx, currentDy, currentRotation),
            BreathBoundary = BoundaryResidual(comparison, current, breath, pivotX, pivotY,
                comparisonDx, comparisonDy, comparisonRotation, currentDx, currentDy, currentRotation),
            FaceLuma = LumaResidual(comparison, current, face, pivotX, pivotY,
                comparisonDx, comparisonDy, comparisonRotation, currentDx, currentDy, currentRotation),
            BreathLuma = LumaResidual(comparison, current, breath, pivotX, pivotY,
                comparisonDx, comparisonDy, comparisonRotation, currentDx, currentDy, currentRotation),
        };
    }

    private static double BoundaryResidual(
        Raster comparison,
        Raster current,
        Region region,
        double pivotX,
        double pivotY,
        double comparisonDx,
        double comparisonDy,
        double comparisonRotationDegrees,
        double currentDx,
        double currentDy,
        double currentRotationDegrees)
    {
        double comparisonRadians = comparisonRotationDegrees * Math.PI / 180.0;
        double comparisonCosine = Math.Cos(comparisonRadians);
        double comparisonSine = Math.Sin(comparisonRadians);
        double currentRadians = currentRotationDegrees * Math.PI / 180.0;
        double currentCosine = Math.Cos(currentRadians);
        double currentSine = Math.Sin(currentRadians);
        int comparisonCount = 0;
        int currentCount = 0;
        int matches = 0;
        for (int y = 1; y < comparison.Height - 1; y++)
        {
            for (int x = 1; x < comparison.Width - 1; x++)
            {
                if (!comparison.Edge[y * comparison.Width + x]) continue;
                double neutralX;
                double neutralY;
                InverseTransform(x, y, pivotX, pivotY,
                    comparisonDx, comparisonDy, comparisonCosine, comparisonSine,
                    out neutralX, out neutralY);
                if (!region.Contains(neutralX, neutralY)) continue;
                comparisonCount++;
                double currentX;
                double currentY;
                Transform(neutralX, neutralY, pivotX, pivotY,
                    currentDx, currentDy, currentCosine, currentSine,
                    out currentX, out currentY);
                if (HasEdge(current, (int)Math.Round(currentX), (int)Math.Round(currentY), 1)) matches++;
            }
        }
        for (int y = 1; y < current.Height - 1; y++)
        {
            for (int x = 1; x < current.Width - 1; x++)
            {
                if (!current.Edge[y * current.Width + x]) continue;
                double neutralX;
                double neutralY;
                InverseTransform(x, y, pivotX, pivotY,
                    currentDx, currentDy, currentCosine, currentSine,
                    out neutralX, out neutralY);
                if (region.Contains(neutralX, neutralY)) currentCount++;
            }
        }
        if (comparisonCount + currentCount == 0) return 0;
        matches = Math.Min(matches, Math.Min(comparisonCount, currentCount));
        return 1.0 - 2.0 * matches / (comparisonCount + currentCount);
    }

    private static double LumaResidual(
        Raster comparison,
        Raster current,
        Region region,
        double pivotX,
        double pivotY,
        double comparisonDx,
        double comparisonDy,
        double comparisonRotationDegrees,
        double currentDx,
        double currentDy,
        double currentRotationDegrees)
    {
        double comparisonRadians = comparisonRotationDegrees * Math.PI / 180.0;
        double comparisonCosine = Math.Cos(comparisonRadians);
        double comparisonSine = Math.Sin(comparisonRadians);
        double currentRadians = currentRotationDegrees * Math.PI / 180.0;
        double currentCosine = Math.Cos(currentRadians);
        double currentSine = Math.Sin(currentRadians);
        double sum = 0;
        int count = 0;
        for (int y = Math.Max(0, (int)Math.Floor(region.Top)); y < Math.Min(comparison.Height, (int)Math.Ceiling(region.Bottom)); y++)
        {
            for (int x = Math.Max(0, (int)Math.Floor(region.Left)); x < Math.Min(comparison.Width, (int)Math.Ceiling(region.Right)); x++)
            {
                double comparisonX;
                double comparisonY;
                Transform(x, y, pivotX, pivotY,
                    comparisonDx, comparisonDy, comparisonCosine, comparisonSine,
                    out comparisonX, out comparisonY);
                double currentX;
                double currentY;
                Transform(x, y, pivotX, pivotY,
                    currentDx, currentDy, currentCosine, currentSine,
                    out currentX, out currentY);
                int bx = (int)Math.Round(comparisonX);
                int by = (int)Math.Round(comparisonY);
                int cx = (int)Math.Round(currentX);
                int cy = (int)Math.Round(currentY);
                if (bx < 0 || by < 0 || bx >= comparison.Width || by >= comparison.Height) continue;
                if (cx < 0 || cy < 0 || cx >= current.Width || cy >= current.Height) continue;
                int comparisonIndex = by * comparison.Width + bx;
                int currentIndex = cy * current.Width + cx;
                if (!comparison.Foreground[comparisonIndex] && !current.Foreground[currentIndex]) continue;
                sum += Math.Abs(comparison.Luma[comparisonIndex] - current.Luma[currentIndex]) / 255.0;
                count++;
            }
        }
        return count == 0 ? 0 : sum / count;
    }

    private static bool HasEdge(Raster raster, int x, int y, int radius)
    {
        for (int yy = Math.Max(1, y - radius); yy <= Math.Min(raster.Height - 2, y + radius); yy++)
            for (int xx = Math.Max(1, x - radius); xx <= Math.Min(raster.Width - 2, x + radius); xx++)
                if (raster.Edge[yy * raster.Width + xx]) return true;
        return false;
    }

    private static void Transform(
        double x, double y, double pivotX, double pivotY, double dx, double dy,
        double cosine, double sine, out double tx, out double ty)
    {
        double relativeX = x - pivotX;
        double relativeY = y - pivotY;
        tx = pivotX + cosine * relativeX - sine * relativeY + dx;
        ty = pivotY + sine * relativeX + cosine * relativeY + dy;
    }

    private static void InverseTransform(
        double x, double y, double pivotX, double pivotY, double dx, double dy,
        double cosine, double sine, out double rx, out double ry)
    {
        double relativeX = x - pivotX - dx;
        double relativeY = y - pivotY - dy;
        rx = pivotX + cosine * relativeX + sine * relativeY;
        ry = pivotY - sine * relativeX + cosine * relativeY;
    }
}
'@

$resolvedFrames = (Resolve-Path -LiteralPath $FramesDir).Path
$resolvedProfile = (Resolve-Path -LiteralPath $ProfilePath).Path
$resolvedReference = (Resolve-Path -LiteralPath $ReferencePath).Path
$files = @(Get-ChildItem -LiteralPath $resolvedFrames -File -Filter "frame-*.jpg" | Sort-Object Name)
if ($files.Count -ne 92) {
    throw "fixed sequence must contain exactly 92 frames; found $($files.Count)"
}

$captureManifestPath = Join-Path $resolvedFrames "capture-manifest.json"
if (-not (Test-Path -LiteralPath $captureManifestPath -PathType Leaf)) {
    throw "fixed sequence is missing capture-manifest.json"
}
$captureManifest = Get-Content -LiteralPath $captureManifestPath -Raw | ConvertFrom-Json
$captureFrames = @($captureManifest.frames)
if ($captureFrames.Count -ne 92) {
    throw "capture manifest must contain exactly 92 frames; found $($captureFrames.Count)"
}
for ($index = 0; $index -lt 92; $index += 1) {
    $capture = $captureFrames[$index]
    $expectedFile = "frame-{0:D3}.jpg" -f $index
    $expectedTarget = $index * 400
    if ($capture.index -ne $index -or $capture.file -ne $expectedFile -or $capture.targetMs -ne $expectedTarget) {
        throw "capture manifest frame $index has an unexpected index, file, or target time"
    }
    if ([Math]::Abs([double] $capture.actualMs - $expectedTarget) -gt 150) {
        throw "capture manifest frame $index exceeds 150 ms jitter"
    }
    $actualHash = (Get-FileHash -LiteralPath $files[$index].FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($capture.sha256 -ne $actualHash) {
        throw "capture manifest frame $index has a mismatched SHA256"
    }
}

$profile = Get-Content -LiteralPath $resolvedProfile -Raw | ConvertFrom-Json
$probe = [System.Drawing.Image]::FromFile($files[0].FullName)
try {
    $width = $probe.Width
    $height = $probe.Height
}
finally {
    $probe.Dispose()
}

$side = [Math]::Min($width, $height)
$imageLeft = ($width - $side) / 2.0
$imageTop = ($height - $side) / 2.0
$alphaHeight = $profile.alphaBounds.bottom - $profile.alphaBounds.top
$faceBottom = $profile.alphaBounds.top + $alphaHeight * 0.4

function Image-X([double] $value) { return $imageLeft + $value * $side }
function Image-Y([double] $value) { return $imageTop + $value * $side }

$metrics = [FixedSequenceMotionAnalyzer]::Analyze(
    [string[]] $files.FullName,
    $resolvedReference,
    (Image-X $profile.alphaBounds.left),
    (Image-Y $profile.alphaBounds.top),
    (Image-X $profile.alphaBounds.right),
    (Image-Y $faceBottom),
    (Image-X $profile.breathZone.left),
    (Image-Y $profile.breathZone.top),
    (Image-X $profile.breathZone.right),
    (Image-Y $profile.breathZone.bottom),
    (Image-X $profile.swayPivot.x),
    (Image-Y $profile.swayPivot.y)
)

function Percentile([double[]] $values, [double] $fraction) {
    $sorted = @($values | Sort-Object)
    if ($sorted.Count -eq 0) { return 0.0 }
    $index = [Math]::Max(0, [Math]::Min($sorted.Count - 1, [Math]::Ceiling($fraction * $sorted.Count) - 1))
    return [double] $sorted[$index]
}

$comparisons = @($metrics)
$faceBoundary = [double[]] @($comparisons.FaceBoundaryResidual)
$breathBoundary = [double[]] @($comparisons.BreathBoundaryResidual)
$faceMedian = Percentile $faceBoundary 0.5
$faceP95 = Percentile $faceBoundary 0.95
$breathMedian = Percentile $breathBoundary 0.5
$breathMinimum = [Math]::Max(0.01, $faceMedian * 1.5)

$result = [ordered]@{
    protocol = [ordered]@{
        frameCount = 92
        targetIntervalMs = 400
        combinedCycleMs = 36400
        foregroundRgbDistanceThreshold = 18
        rigidDxRangePixels = @(-4, 4)
        rigidDyRangePixels = @(-4, 4)
        rigidRotationRangeDegrees = @(-0.8, 0.8)
        rigidRotationStepDegrees = 0.1
        faceBoundaryP95Maximum = 0.03
        breathBoundaryMedianMinimum = 0.01
        breathToFaceMedianMinimum = 1.5
    }
    dimensions = [ordered]@{ width = $width; height = $height }
    captureManifest = [ordered]@{
        path = [IO.Path]::GetFileName($captureManifestPath)
        maxJitterMs = [double] (($captureFrames | ForEach-Object {
            [Math]::Abs([double] $_.actualMs - [double] $_.targetMs)
        } | Measure-Object -Maximum).Maximum)
    }
    neutralReference = [ordered]@{
        path = [IO.Path]::GetFileName($resolvedReference)
        sha256 = (Get-FileHash -LiteralPath $resolvedReference -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    summary = [ordered]@{
        faceBoundaryMedian = $faceMedian
        faceBoundaryP95 = $faceP95
        breathBoundaryMedian = $breathMedian
        requiredBreathBoundaryMedian = $breathMinimum
        faceSafetyPassed = $faceP95 -le 0.03
        breathZonePassed = $breathMedian -ge $breathMinimum
        passed = ($faceP95 -le 0.03) -and ($breathMedian -ge $breathMinimum)
    }
    frames = $metrics
}

$json = $result | ConvertTo-Json -Depth 8
[System.IO.File]::WriteAllText($OutputPath, $json, [System.Text.UTF8Encoding]::new($false))
Write-Output $json
