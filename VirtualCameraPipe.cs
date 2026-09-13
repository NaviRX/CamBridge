using System.Drawing.Imaging;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Threading.Channels;

namespace CamBridge;

// One-way local frame transport to the Frame Server media-source DLL.
internal sealed class VirtualCameraPipe
{
    public const int Width = 1920;
    public const int Height = 1080;
    public const string Name = "CamBridge.Video.v1";
    readonly Channel<byte[]> frames = Channel.CreateBounded<byte[]>(new BoundedChannelOptions(1)
    { SingleReader = true, SingleWriter = true, FullMode = BoundedChannelFullMode.DropOldest });
    volatile bool connected;

    public void Post(Bitmap input)
    {
        if (!connected) return;
        using var scaled = new Bitmap(Width, Height, PixelFormat.Format32bppRgb);
        using (var graphics = Graphics.FromImage(scaled))
        {
            graphics.InterpolationMode = System.Drawing.Drawing2D.InterpolationMode.HighQualityBilinear;
            graphics.DrawImage(input, new Rectangle(0, 0, Width, Height));
        }
        var bytes = new byte[Width * Height * 4];
        var bits = scaled.LockBits(new Rectangle(0, 0, Width, Height), ImageLockMode.ReadOnly, scaled.PixelFormat);
        try
        {
            for (int y = 0; y < Height; y++)
                Marshal.Copy(IntPtr.Add(bits.Scan0, y * bits.Stride), bytes, (Height - 1 - y) * Width * 4, Width * 4);
        }
        finally { scaled.UnlockBits(bits); }
        frames.Writer.TryWrite(bytes);
    }

    public async Task RunAsync(CancellationToken token)
    {
        while (!token.IsCancellationRequested)
        {
            try
            {
                using var pipe = new NamedPipeServerStream(Name, PipeDirection.Out, 1,
                    PipeTransmissionMode.Byte, PipeOptions.Asynchronous);
                await pipe.WaitForConnectionAsync(token);
                connected = true;
                while (pipe.IsConnected && !token.IsCancellationRequested)
                {
                    var frame = await frames.Reader.ReadAsync(token);
                    await pipe.WriteAsync(BitConverter.GetBytes(frame.Length), token);
                    await pipe.WriteAsync(frame, token);
                    await pipe.FlushAsync(token);
                }
            }
            catch (OperationCanceledException) { break; }
            catch (IOException) { /* The camera consumer closed; wait for the next one. */ }
            finally { connected = false; }
        }
    }
}
