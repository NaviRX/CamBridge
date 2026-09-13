using System.Drawing.Imaging;
using System.Runtime.InteropServices;

namespace CamBridge;

internal sealed record CameraDevice(string Name, string Link)
{
    public override string ToString() => Name;
}

internal sealed record CameraFormat(int Width, int Height, int Fps, Guid Subtype, int NativeIndex)
{
    public override string ToString()
    {
        var fourcc = Subtype.ToString()[..8];
        var name = fourcc switch
        {
            "47504a4d" => "MJPEG",
            "3231564e" => "NV12",
            "34363248" => "H.264",
            "32595559" => "YUY2",
            _ => Subtype.ToString()
        };
        return $"{Width}×{Height} · {Fps} fps · {name}";
    }
}

// Uses only the Windows Media Foundation DLLs shipped with Windows.
internal sealed class MediaFoundationCamera : IDisposable
{
    const uint VideoStream = 0xfffffffc;
    static readonly Guid SourceType = new("c60ac5fe-252a-478f-a0ef-bc8fa5f7cad3");
    static readonly Guid VideoCapture = new("8ac3587a-4ae7-42d8-99e0-0a6013eef90f");
    static readonly Guid FriendlyName = new("60d0e559-52f8-4fa2-bbce-acdb34a8ec01");
    static readonly Guid SymbolicLink = new("58f0aad8-22bf-4f8a-bb3d-d2c4978c6e2f");
    static readonly Guid Shared = new("1cb378e9-b279-41d4-af97-34a243e68320");
    static readonly Guid Processing = new("fb394f3d-ccf1-42ee-bbb3-f9b845d5681d");
    static readonly Guid Subtype = new("f7e34c9a-42e8-4714-b74b-cb29d72c35e5");
    static readonly Guid FrameSize = new("1652c33d-d6b2-4012-b834-72030849a37d");
    static readonly Guid FrameRate = new("c459a2e8-3d2c-4e44-b132-fee5156c7bb0");
    static readonly Guid Rgb32 = new("00000016-0000-0010-8000-00aa00389b71");
    static readonly Guid VideoMajor = new("73646976-0000-0010-8000-00aa00389b71");
    static readonly Guid MajorType = new("48eba18e-f8c9-4687-bf11-0a74c9f96a8f");

    IntPtr source;
    IntPtr reader;
    int width;
    int height;

    MediaFoundationCamera(IntPtr source, IntPtr reader, int width, int height)
    { this.source = source; this.reader = reader; this.width = width; this.height = height; }

    public static IReadOnlyList<CameraDevice> Enumerate()
    {
        Check(MFStartup(0x20070, 0));
        IntPtr attributes = IntPtr.Zero, array = IntPtr.Zero;
        try
        {
            Check(MFCreateAttributes(out attributes, 1));
            SetGuid(attributes, SourceType, VideoCapture);
            Check(MFEnumDeviceSources(attributes, out array, out var count));
            var result = new List<CameraDevice>();
            for (int i = 0; i < count; i++)
            {
                var activate = Marshal.ReadIntPtr(array, i * IntPtr.Size);
                try { result.Add(new(GetString(activate, FriendlyName), GetString(activate, SymbolicLink))); }
                finally { Release(activate); }
            }
            return result;
        }
        finally { if (array != IntPtr.Zero) Marshal.FreeCoTaskMem(array); Release(attributes); MFShutdown(); }
    }

    public static IReadOnlyList<CameraFormat> Formats(CameraDevice device)
    {
        using var camera = OpenReader(device);
        var result = new List<CameraFormat>();
        for (uint index = 0; index < 256; index++)
        {
            var hr = Method<GetNativeMediaType>(camera.reader, 5)(camera.reader, VideoStream, index, out var type);
            if (hr != 0) break;
            try
            {
                var size = GetUInt64(type, FrameSize);
                var rate = GetUInt64(type, FrameRate);
                var subtype = GetGuid(type, Subtype);
                int width = (int)(size >> 32), height = (int)(size & 0xffffffff);
                int fps = (int)Math.Round((rate >> 32) / (double)Math.Max(1, rate & 0xffffffff));
                if (width > 0 && height > 0 && fps > 0)
                    result.Add(new(width, height, fps, subtype, (int)index));
            }
            finally { Release(type); }
        }
        return result;
    }

    public static MediaFoundationCamera Open(CameraDevice device, CameraFormat format)
    {
        var camera = OpenReader(device);
        try
        {
            Check(Method<GetNativeMediaType>(camera.reader, 5)(camera.reader, VideoStream, (uint)format.NativeIndex, out var native));
            try { Check(Method<SetCurrentMediaType>(camera.reader, 7)(camera.reader, VideoStream, IntPtr.Zero, native)); }
            finally { Release(native); }
            Check(MFCreateMediaType(out var output));
            try
            {
                SetGuid(output, MajorType, VideoMajor);
                SetGuid(output, Subtype, Rgb32);
                SetUInt64(output, FrameSize, ((ulong)(uint)format.Width << 32) | (uint)format.Height);
                SetUInt64(output, FrameRate, ((ulong)(uint)format.Fps << 32) | 1);
                Check(Method<SetCurrentMediaType>(camera.reader, 7)(camera.reader, VideoStream, IntPtr.Zero, output));
            }
            finally { Release(output); }
            camera.width = format.Width; camera.height = format.Height;
            return camera;
        }
        catch { camera.Dispose(); throw; }
    }

    static MediaFoundationCamera OpenReader(CameraDevice device)
    {
        Check(MFStartup(0x20070, 0));
        IntPtr attributes = IntPtr.Zero, source = IntPtr.Zero, reader = IntPtr.Zero, options = IntPtr.Zero;
        try
        {
            Check(MFCreateAttributes(out attributes, 3));
            SetGuid(attributes, SourceType, VideoCapture);
            var link = SymbolicLink;
            Check(Method<SetString>(attributes, 25)(attributes, ref link, device.Link));
            var shared = Shared;
            Check(Method<SetUInt32>(attributes, 21)(attributes, ref shared, 1));
            Check(MFCreateDeviceSource(attributes, out source));
            Check(MFCreateAttributes(out options, 1));
            var processing = Processing;
            Check(Method<SetUInt32>(options, 21)(options, ref processing, 1));
            Check(MFCreateSourceReaderFromMediaSource(source, options, out reader));
            return new(source, reader, 0, 0);
        }
        catch { Release(reader); Release(source); MFShutdown(); throw; }
        finally { Release(options); Release(attributes); }
    }

    public Bitmap ReadFrame()
    {
        IntPtr sample = IntPtr.Zero;
        for (int attempt = 0; attempt < 30 && sample == IntPtr.Zero; attempt++)
        {
            Check(Method<ReadSample>(reader, 9)(reader, VideoStream, 0, out _, out var flags, out _, out sample));
            if ((flags & 0x00000002) != 0) throw new IOException("카메라 스트림이 종료되었습니다.");
        }
        if (sample == IntPtr.Zero) throw new IOException("카메라에서 프레임이 도착하지 않았습니다.");
        try
        {
            Check(Method<ConvertBuffer>(sample, 41)(sample, out var buffer));
            try
            {
                Check(Method<LockBuffer>(buffer, 3)(buffer, out var data, out _, out var length));
                try
                {
                    if (length < width * height * 4) throw new IOException("카메라 프레임 크기가 예상보다 작습니다.");
                    var bitmap = new Bitmap(width, height, PixelFormat.Format32bppRgb);
                    var bits = bitmap.LockBits(new Rectangle(0, 0, width, height), ImageLockMode.WriteOnly, bitmap.PixelFormat);
                    try
                    {
                        for (int y = 0; y < height; y++)
                            CopyMemory(IntPtr.Add(bits.Scan0, y * bits.Stride), IntPtr.Add(data, y * width * 4), width * 4);
                    }
                    finally { bitmap.UnlockBits(bits); }
                    return bitmap;
                }
                finally { Check(Method<UnlockBuffer>(buffer, 4)(buffer)); }
            }
            finally { Release(buffer); }
        }
        finally { Release(sample); }
    }

    public void Dispose()
    {
        if (reader == IntPtr.Zero && source == IntPtr.Zero) return;
        Release(reader); Release(source); reader = source = IntPtr.Zero; MFShutdown();
    }

    static unsafe void CopyMemory(IntPtr target, IntPtr source, int bytes)
        => Buffer.MemoryCopy((void*)source, (void*)target, bytes, bytes);
    static void Check(int hr) { if (hr < 0) Marshal.ThrowExceptionForHR(hr); }
    static void Release(IntPtr ptr) { if (ptr != IntPtr.Zero) Marshal.Release(ptr); }
    static T Method<T>(IntPtr obj, int index) where T : Delegate => Marshal.GetDelegateForFunctionPointer<T>(Marshal.ReadIntPtr(Marshal.ReadIntPtr(obj), index * IntPtr.Size));
    static void SetGuid(IntPtr obj, Guid key, Guid value) => Check(Method<SetGuidDelegate>(obj, 24)(obj, ref key, ref value));
    static Guid GetGuid(IntPtr obj, Guid key) { Check(Method<GetGuidDelegate>(obj, 10)(obj, ref key, out var value)); return value; }
    static ulong GetUInt64(IntPtr obj, Guid key) { Check(Method<GetUInt64Delegate>(obj, 8)(obj, ref key, out var value)); return value; }
    static void SetUInt64(IntPtr obj, Guid key, ulong value) => Check(Method<SetUInt64Delegate>(obj, 22)(obj, ref key, value));
    static string GetString(IntPtr obj, Guid key)
    {
        Check(Method<GetAllocatedString>(obj, 13)(obj, ref key, out var value, out _));
        try { return Marshal.PtrToStringUni(value) ?? ""; }
        finally { Marshal.FreeCoTaskMem(value); }
    }

    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int SetGuidDelegate(IntPtr self, ref Guid key, ref Guid value);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int GetGuidDelegate(IntPtr self, ref Guid key, out Guid value);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int GetUInt64Delegate(IntPtr self, ref Guid key, out ulong value);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int SetUInt64Delegate(IntPtr self, ref Guid key, ulong value);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int SetUInt32(IntPtr self, ref Guid key, uint value);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int GetAllocatedString(IntPtr self, ref Guid key, out IntPtr value, out uint length);
    [UnmanagedFunctionPointer(CallingConvention.StdCall, CharSet = CharSet.Unicode)] delegate int SetString(IntPtr self, ref Guid key, [MarshalAs(UnmanagedType.LPWStr)] string value);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int GetNativeMediaType(IntPtr self, uint stream, uint index, out IntPtr type);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int SetCurrentMediaType(IntPtr self, uint stream, IntPtr reserved, IntPtr type);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int ReadSample(IntPtr self, uint stream, uint control, out uint actual, out uint flags, out long timestamp, out IntPtr sample);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int ConvertBuffer(IntPtr self, out IntPtr buffer);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int LockBuffer(IntPtr self, out IntPtr data, out uint max, out uint length);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int UnlockBuffer(IntPtr self);

    [DllImport("mfplat.dll", ExactSpelling = true)] static extern int MFStartup(int version, int flags);
    [DllImport("mfplat.dll", ExactSpelling = true)] static extern int MFShutdown();
    [DllImport("mfplat.dll", ExactSpelling = true)] static extern int MFCreateAttributes(out IntPtr attributes, uint initialSize);
    [DllImport("mfplat.dll", ExactSpelling = true)] static extern int MFCreateMediaType(out IntPtr type);
    [DllImport("mf.dll", ExactSpelling = true)] static extern int MFEnumDeviceSources(IntPtr attributes, out IntPtr activates, out uint count);
    [DllImport("mf.dll", ExactSpelling = true)] static extern int MFCreateDeviceSource(IntPtr attributes, out IntPtr source);
    [DllImport("mfreadwrite.dll", ExactSpelling = true)] static extern int MFCreateSourceReaderFromMediaSource(IntPtr source, IntPtr attributes, out IntPtr reader);
}
