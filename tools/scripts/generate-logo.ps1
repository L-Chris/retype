# Generate raster icons from the simple geometry in assets/logo.svg.
# The SVG remains the editable brand source; keep coordinates/colors in sync.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function New-LogoPng([int]$size) {
  $bitmap = New-Object Drawing.Bitmap($size, $size)
  $graphics = [Drawing.Graphics]::FromImage($bitmap)
  $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
  $graphics.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  $graphics.Clear([Drawing.Color]::Transparent)
  $graphics.ScaleTransform($size / 128.0, $size / 128.0)
  $shape = New-Object Drawing.Drawing2D.GraphicsPath
  $shape.AddArc(0, 0, 56, 56, 180, 90)
  $shape.AddArc(72, 0, 56, 56, 270, 90)
  $shape.AddArc(72, 72, 56, 56, 0, 90)
  $shape.AddArc(0, 72, 56, 56, 90, 90)
  $shape.CloseFigure()
  $background = New-Object Drawing.SolidBrush([Drawing.Color]::FromArgb(20, 43, 59))
  $graphics.FillPath($background, $shape)
  $white = New-Object Drawing.Pen([Drawing.Color]::FromArgb(248, 251, 255), 14)
  $white.StartCap = [Drawing.Drawing2D.LineCap]::Round
  $white.EndCap = [Drawing.Drawing2D.LineCap]::Round
  $white.LineJoin = [Drawing.Drawing2D.LineJoin]::Round
  $graphics.DrawLine($white, 42, 48, 42, 88)
  $graphics.DrawBezier($white, 47, 62, 56, 45, 67, 42, 79, 49)
  $cyan = New-Object Drawing.Pen([Drawing.Color]::FromArgb(84, 214, 206), 8)
  $cyan.StartCap = [Drawing.Drawing2D.LineCap]::Round
  $cyan.EndCap = [Drawing.Drawing2D.LineCap]::Round
  $graphics.DrawLine($cyan, 86, 68, 86, 87)
  $stream = New-Object IO.MemoryStream
  $bitmap.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
  $bytes = $stream.ToArray()
  $stream.Dispose(); $cyan.Dispose(); $white.Dispose(); $background.Dispose()
  $shape.Dispose(); $graphics.Dispose(); $bitmap.Dispose()
  return ,$bytes
}

$flutterPath = Join-Path $root 'apps\settings\assets\logo.png'
New-Item -ItemType Directory -Force -Path (Split-Path $flutterPath) | Out-Null
[IO.File]::WriteAllBytes($flutterPath, (New-LogoPng 256))

$android = Join-Path $root 'apps\settings\android\app\src\main\res'
foreach ($entry in @(@('mdpi', 48), @('hdpi', 72), @('xhdpi', 96), @('xxhdpi', 144), @('xxxhdpi', 192))) {
  $path = Join-Path $android ("mipmap-{0}\ic_launcher.png" -f $entry[0])
  [IO.File]::WriteAllBytes($path, (New-LogoPng ([int]$entry[1])))
}

$sizes = @(16, 20, 24, 32, 40, 48, 64, 128, 256)
$frames = @($sizes | ForEach-Object { New-LogoPng $_ })
$iconPath = Join-Path $root 'apps\settings\windows\runner\resources\app_icon.ico'
$file = [IO.File]::Create($iconPath)
$writer = New-Object IO.BinaryWriter($file)
try {
  $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$frames.Count)
  $offset = 6 + 16 * $frames.Count
  for ($i = 0; $i -lt $frames.Count; $i++) {
    $writer.Write([byte]($sizes[$i] % 256)); $writer.Write([byte]($sizes[$i] % 256))
    $writer.Write([byte]0); $writer.Write([byte]0)
    $writer.Write([uint16]1); $writer.Write([uint16]32)
    $writer.Write([uint32]$frames[$i].Length); $writer.Write([uint32]$offset)
    $offset += $frames[$i].Length
  }
  foreach ($frame in $frames) { $writer.Write([byte[]]$frame) }
} finally { $writer.Dispose() }
Write-Host "Generated retype logo assets from assets/logo.svg"
