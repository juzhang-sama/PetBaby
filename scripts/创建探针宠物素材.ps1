param([string]$OutputPath = 'public/test-assets/pet-probe.png')

Add-Type -AssemblyName System.Drawing
$resolved = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $OutputPath))
$directory = [System.IO.Path]::GetDirectoryName($resolved)
[System.IO.Directory]::CreateDirectory($directory) | Out-Null
$bitmap = [System.Drawing.Bitmap]::new(512, 512, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$graphics.Clear([System.Drawing.Color]::Transparent)
$fur = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255,72,94,86))
$inner = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255,220,174,169))
$eye = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255,245,205,74))
$graphics.FillEllipse($fur,106,154,300,324)
$graphics.FillEllipse($fur,126,68,260,236)
$graphics.FillPolygon($fur,[System.Drawing.Point[]]@((New-Object System.Drawing.Point 145,118),(New-Object System.Drawing.Point 168,28),(New-Object System.Drawing.Point 222,94)))
$graphics.FillPolygon($fur,[System.Drawing.Point[]]@((New-Object System.Drawing.Point 290,94),(New-Object System.Drawing.Point 344,28),(New-Object System.Drawing.Point 367,118)))
$graphics.FillEllipse($inner,176,159,55,44)
$graphics.FillEllipse($inner,281,159,55,44)
$graphics.FillEllipse($eye,190,171,22,18)
$graphics.FillEllipse($eye,300,171,22,18)
$bitmap.Save($resolved,[System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose(); $bitmap.Dispose(); $fur.Dispose(); $inner.Dispose(); $eye.Dispose()
Write-Output $resolved
