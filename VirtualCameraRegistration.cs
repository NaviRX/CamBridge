using System.Runtime.InteropServices;

namespace CamBridge;

internal sealed class VirtualCameraRegistration : IDisposable
{
    public const string SourceClsid = "{1483AAF4-E019-46C7-BFDC-322077BA141F}";
    IntPtr camera;

    VirtualCameraRegistration(IntPtr camera) => this.camera = camera;

    public static VirtualCameraRegistration Start()
    {
        if (Environment.OSVersion.Version.Build < 22000)
            throw new PlatformNotSupportedException("Windows Media Foundation 가상 카메라는 Windows 11 빌드 22000 이상이 필요합니다.");
        var hr = MFStartup(0x20070, 0);
        if (hr < 0) Marshal.ThrowExceptionForHR(hr);
        IntPtr camera = IntPtr.Zero;
        try
        {
            hr = MFCreateVirtualCamera(0, 1, 0, "CamBridge", SourceClsid, IntPtr.Zero, 0, out camera);
            if (hr < 0) Marshal.ThrowExceptionForHR(hr);
            hr = Method<StartDelegate>(camera, 36)(camera, IntPtr.Zero);
            if (hr < 0) Marshal.ThrowExceptionForHR(hr);
            return new(camera);
        }
        catch
        {
            if (camera != IntPtr.Zero) Marshal.Release(camera);
            MFShutdown();
            throw;
        }
    }

    public void Dispose()
    {
        if (camera == IntPtr.Zero) return;
        Method<StopDelegate>(camera, 37)(camera);
        Marshal.Release(camera);
        camera = IntPtr.Zero;
        MFShutdown();
    }

    static T Method<T>(IntPtr obj, int index) where T : Delegate
        => Marshal.GetDelegateForFunctionPointer<T>(Marshal.ReadIntPtr(Marshal.ReadIntPtr(obj), index * IntPtr.Size));

    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int StartDelegate(IntPtr self, IntPtr callback);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int StopDelegate(IntPtr self);
    [DllImport("mfplat.dll", ExactSpelling = true)] static extern int MFStartup(int version, int flags);
    [DllImport("mfplat.dll", ExactSpelling = true)] static extern int MFShutdown();
    [DllImport("mfsensorgroup.dll", ExactSpelling = true, CharSet = CharSet.Unicode)]
    static extern int MFCreateVirtualCamera(int type, int lifetime, int access,
        [MarshalAs(UnmanagedType.LPWStr)] string friendlyName,
        [MarshalAs(UnmanagedType.LPWStr)] string sourceId,
        IntPtr categories, uint categoryCount, out IntPtr virtualCamera);
}
