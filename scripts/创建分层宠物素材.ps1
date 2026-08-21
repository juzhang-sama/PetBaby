param([string]$OutputDir = 'public/test-assets/layered')

Add-Type -AssemblyName System.Drawing
$resolved = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $OutputDir))
[System.IO.Directory]::CreateDirectory($resolved) | Out-Null

function Save-LayerPng {
  param([System.Drawing.Bitmap]$Bitmap, [string]$Name)
  $path = Join-Path $resolved $Name
  $Bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $Bitmap.Dispose()
  Write-Output $path
}

# body: 身体+头，无眼睛
$body = [System.Drawing.Bitmap]::new(512, 512, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($body)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)
$fur = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255,72,94,86))
$inner = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255,220,174,169))
$g.FillEllipse($fur,106,154,300,324)
$g.FillEllipse($fur,126,68,260,236)
$g.FillPolygon($fur,[System.Drawing.Point[]]@((New-Object System.Drawing.Point 145,118),(New-Object System.Drawing.Point 168,28),(New-Object System.Drawing.Point 222,94)))
$g.FillPolygon($fur,[System.Drawing.Point[]]@((New-Object System.Drawing.Point 290,94),(New-Object System.Drawing.Point 344,28),(New-Object System.Drawing.Point 367,118)))
$g.FillEllipse($inner,176,159,55,44)
$g.FillEllipse($inner,281,159,55,44)
$g.Dispose(); $fur.Dispose(); $inner.Dispose()
Save-LayerPng $body 'body.png' | Out-Null

# eye-open: 睁眼（瞳孔+高光）
$open = [System.Drawing.Bitmap]::new(512, 512, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($open)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)
$eye = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255,245,205,74))
$pupil = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255,40,40,40))
$g.FillEllipse($eye,190,171,22,18)
$g.FillEllipse($eye,300,171,22,18)
$g.FillEllipse($pupil,196,175,10,10)
$g.FillEllipse($pupil,306,175,10,10)
$g.Dispose(); $eye.Dispose(); $pupil.Dispose()
Save-LayerPng $open 'eye-open.png' | Out-Null

# eye-closed: 闭眼线条
$closed = [System.Drawing.Bitmap]::new(512, 512, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($closed)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)
$pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255,60,60,60)), 4
$g.DrawArc($pen,186,172,30,16,20,140)
$g.DrawArc($pen,296,172,30,16,20,140)
$g.Dispose(); $pen.Dispose()
Save-LayerPng $closed 'eye-closed.png' | Out-Null

# accent: 腮红装饰（点击反馈可见变化）
$accent = [System.Drawing.Bitmap]::new(512, 512, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($accent)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)
$blush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(160,255,120,120))
$g.FillEllipse($blush,150,220,40,24)
$g.FillEllipse($blush,322,220,40,24)
$g.Dispose(); $blush.Dispose()
Save-LayerPng $accent 'accent.png' | Out-Null

Write-Output "layered assets written to $resolved"
