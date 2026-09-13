param([string]$Output = (Join-Path $PSScriptRoot '..\Assets\CamBridge.ico'))

Add-Type -AssemblyName System.Drawing

$sizes = @(16, 20, 24, 32, 40, 48, 64, 256)
$frames = [System.Collections.Generic.List[byte[]]]::new()

foreach ($size in $sizes) {
    $bitmap = [Drawing.Bitmap]::new($size, $size, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.Clear([Drawing.Color]::Transparent)

    $color = [Drawing.Color]::FromArgb(255, 35, 123, 142)
    $brush = [Drawing.SolidBrush]::new($color)
    $scale = $size / 16.0
    $body = [Drawing.RectangleF]::new(1.5 * $scale, 4.0 * $scale, 9.5 * $scale, 8.0 * $scale)
    $radius = 2.0 * $scale
    $path = [Drawing.Drawing2D.GraphicsPath]::new()
    $path.AddArc($body.X, $body.Y, $radius, $radius, 180, 90)
    $path.AddArc($body.Right - $radius, $body.Y, $radius, $radius, 270, 90)
    $path.AddArc($body.Right - $radius, $body.Bottom - $radius, $radius, $radius, 0, 90)
    $path.AddArc($body.X, $body.Bottom - $radius, $radius, $radius, 90, 90)
    $path.CloseFigure()
    $graphics.FillPath($brush, $path)

    $lens = [Drawing.PointF[]]@(
        [Drawing.PointF]::new(10.0 * $scale, 6.0 * $scale),
        [Drawing.PointF]::new(14.5 * $scale, 3.8 * $scale),
        [Drawing.PointF]::new(14.5 * $scale, 12.2 * $scale),
        [Drawing.PointF]::new(10.0 * $scale, 10.0 * $scale)
    )
    $graphics.FillPolygon($brush, $lens)

    $stream = [IO.MemoryStream]::new()
    $bitmap.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
    $frames.Add($stream.ToArray())
    $stream.Dispose(); $path.Dispose(); $brush.Dispose(); $graphics.Dispose(); $bitmap.Dispose()
}

$directory = Split-Path -Parent $Output
[IO.Directory]::CreateDirectory($directory) | Out-Null
$file = [IO.File]::Create($Output)
$writer = [IO.BinaryWriter]::new($file)
$writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$frames.Count)
$offset = 6 + (16 * $frames.Count)
for ($i = 0; $i -lt $frames.Count; $i++) {
    $size = $sizes[$i]
    $writer.Write([byte]$(if ($size -eq 256) { 0 } else { $size }))
    $writer.Write([byte]$(if ($size -eq 256) { 0 } else { $size }))
    $writer.Write([byte]0); $writer.Write([byte]0)
    $writer.Write([uint16]1); $writer.Write([uint16]32)
    $writer.Write([uint32]$frames[$i].Length); $writer.Write([uint32]$offset)
    $offset += $frames[$i].Length
}
foreach ($frame in $frames) { $writer.Write($frame) }
$writer.Dispose(); $file.Dispose()
