using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Net.NetworkInformation;
using Microsoft.Win32;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

namespace CamBridge;

internal static class Program
{
    [STAThread]
    static void Main()
    {
        ApplicationConfiguration.Initialize();
        Application.Run(new MainForm());
    }
}

internal sealed class MainForm : Form
{
    const int VideoPort = 45831;
    const int DiscoveryPort = 45832;
    readonly ComboBox mode = new() { DropDownStyle = ComboBoxStyle.DropDownList };
    readonly ComboBox camera = new() { DropDownStyle = ComboBoxStyle.DropDownList };
    readonly ComboBox cameraFormat = new() { DropDownStyle = ComboBoxStyle.DropDownList };
    readonly Button refreshCameras = new() { Text = "장치 새로고침" };
    readonly TextBox remoteIp = new() { PlaceholderText = "예: 192.168.0.33" };
    readonly Label senderAddress = new() { AutoSize = true };
    readonly NumericUpDown fps = new() { Minimum = 5, Maximum = 60, Value = 30 };
    readonly NumericUpDown quality = new() { Minimum = 30, Maximum = 95, Value = 75 };
    readonly CheckBox testPattern = new() { Text = "테스트 패턴 사용" };
    readonly CheckBox compatibilityMode = new() { Text = "OBS 호환 모드 (로컬 가상 카메라)", AutoSize = true };
    readonly CheckBox startup = new() { Text = "Windows 시작 시 실행" };
    readonly Button toggle = new() { Text = "시작", Height = 36 };
    readonly Label status = new() { Text = "중지됨", AutoSize = true };
    readonly PictureBox preview = new() { Dock = DockStyle.Fill, SizeMode = PictureBoxSizeMode.Zoom, BackColor = Color.Black };
    readonly Icon appIcon = LoadAppIcon();
    readonly NotifyIcon tray = new() { Text = "CamBridge", Visible = true };
    CancellationTokenSource? cts;
    Task? bridgeTask;
    long sentFrames;
    long receivedFrames;
    DateTime statsAt = DateTime.UtcNow;

    public MainForm()
    {
        Text = "CamBridge — 네트워크 웹캠";
        Icon = appIcon;
        tray.Icon = appIcon;
        Width = 900; Height = 610; MinimumSize = new Size(720, 480);
        mode.Items.AddRange(["송신", "수신"]); mode.SelectedIndex = 0;
        startup.Checked = IsStartupEnabled();

        var settings = new TableLayoutPanel { Dock = DockStyle.Left, Width = 270, Padding = new Padding(14), ColumnCount = 2, AutoScroll = true };
        settings.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 44));
        settings.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 56));
        AddRow(settings, "동작 모드", mode);
        AddRow(settings, "카메라", camera);
        AddRow(settings, "캡처 형식", cameraFormat);
        settings.Controls.Add(refreshCameras, 0, settings.RowCount); settings.SetColumnSpan(refreshCameras, 2); settings.RowCount++;
        AddRow(settings, "이 PC 송신 IP", senderAddress);
        AddRow(settings, "수신 IP", remoteIp);
        senderAddress.Text = LocalAddresses();
        AddRow(settings, "FPS", fps);
        AddRow(settings, "JPEG 품질", quality);
        settings.Controls.Add(testPattern, 0, settings.RowCount); settings.SetColumnSpan(testPattern, 2); settings.RowCount++;
        settings.Controls.Add(compatibilityMode, 0, settings.RowCount); settings.SetColumnSpan(compatibilityMode, 2); settings.RowCount++;
        settings.Controls.Add(startup, 0, settings.RowCount); settings.SetColumnSpan(startup, 2); settings.RowCount++;
        settings.Controls.Add(toggle, 0, settings.RowCount); settings.SetColumnSpan(toggle, 2); settings.RowCount++;
        settings.Controls.Add(status, 0, settings.RowCount); settings.SetColumnSpan(status, 2); settings.RowCount++;
        Controls.Add(preview); Controls.Add(settings);

        var menu = new ContextMenuStrip();
        menu.Items.Add("열기", null, (_, _) => ShowFromTray());
        menu.Items.Add("시작/중지", null, async (_, _) => await ToggleAsync());
        menu.Items.Add("종료", null, async (_, _) => { await StopAsync(); tray.Visible = false; Application.Exit(); });
        tray.ContextMenuStrip = menu;
        tray.DoubleClick += (_, _) => ShowFromTray();
        toggle.Click += async (_, _) => await ToggleAsync();
        refreshCameras.Click += async (_, _) => await RefreshCamerasAsync();
        camera.SelectedIndexChanged += async (_, _) => await RefreshFormatsAsync();
        cameraFormat.SelectedIndexChanged += (_, _) =>
        {
            if (cameraFormat.SelectedItem is CameraFormat format)
                fps.Value = Math.Clamp(format.Fps, (int)fps.Minimum, (int)fps.Maximum);
        };
        startup.CheckedChanged += (_, _) => SetStartup(startup.Checked);
        mode.SelectedIndexChanged += (_, _) =>
        {
            remoteIp.Enabled = mode.SelectedIndex == 1;
            remoteIp.Visible = mode.SelectedIndex == 1;
            senderAddress.Visible = mode.SelectedIndex == 0;
            compatibilityMode.Enabled = mode.SelectedIndex == 0;
        };
        remoteIp.Enabled = false;
        remoteIp.Visible = false;
        FormClosing += async (_, e) =>
        {
            if (e.CloseReason == CloseReason.UserClosing) { e.Cancel = true; Hide(); tray.ShowBalloonTip(1200, "CamBridge", "트레이에서 계속 실행 중입니다.", ToolTipIcon.Info); }
            else await StopAsync();
        };
        Shown += async (_, _) => await RefreshCamerasAsync();
    }

    static Icon LoadAppIcon()
    {
        var iconPath = Path.Combine(AppContext.BaseDirectory, "Assets", "CamBridge.ico");
        return File.Exists(iconPath) ? new Icon(iconPath) : (Icon)SystemIcons.Application.Clone();
    }

    static void AddRow(TableLayoutPanel panel, string label, Control control)
    {
        var row = panel.RowCount++;
        panel.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        panel.Controls.Add(new Label { Text = label, AutoSize = true, Anchor = AnchorStyles.Left, Margin = new Padding(3, 9, 3, 3) }, 0, row);
        control.Dock = DockStyle.Top;
        panel.Controls.Add(control, 1, row);
    }

    async Task ToggleAsync()
    {
        if (cts is not null) await StopAsync();
        else StartBridge();
    }

    void StartBridge()
    {
        cts = new CancellationTokenSource();
        toggle.Text = "중지";
        LockSettings(true);
        sentFrames = receivedFrames = 0; statsAt = DateTime.UtcNow;
        bridgeTask = mode.SelectedIndex == 0 ? RunSenderAsync(cts.Token) : RunReceiverAsync(cts.Token);
    }

    async Task StopAsync()
    {
        var old = cts;
        if (old is null) return;
        cts = null; old.Cancel();
        if (bridgeTask is not null) await bridgeTask;
        bridgeTask = null;
        old.Dispose();
        if (!IsDisposed) BeginInvoke(() => { toggle.Text = "시작"; status.Text = "중지됨"; LockSettings(false); });
    }

    void LockSettings(bool running)
    {
        mode.Enabled = camera.Enabled = cameraFormat.Enabled = refreshCameras.Enabled = fps.Enabled = quality.Enabled = testPattern.Enabled = !running;
        compatibilityMode.Enabled = !running && mode.SelectedIndex == 0;
        remoteIp.Enabled = !running && mode.SelectedIndex == 1;
    }

    async Task RunSenderAsync(CancellationToken token)
    {
        var selectedDevice = camera.SelectedItem as CameraDevice;
        var selectedFormat = cameraFormat.SelectedItem as CameraFormat;
        var useTestPattern = testPattern.Checked;
        var selectedFps = (int)fps.Value;
        var selectedQuality = (int)quality.Value;
        var localVirtualCamera = compatibilityMode.Checked;
        try
        {
            using var udp = new UdpClient(DiscoveryPort);
            var peer = new DiscoveryPeer();
            using var discoveryCts = CancellationTokenSource.CreateLinkedTokenSource(token);
            var discoveryTask = ListenForReceiverAsync(udp, peer, discoveryCts.Token);
            if (!useTestPattern && (selectedDevice is null || selectedFormat is null))
                throw new InvalidOperationException("실제 카메라와 캡처 형식을 선택하세요.");
            VirtualCameraPipe? localPipe = null;
            VirtualCameraRegistration? localRegistration = null;
            if (localVirtualCamera)
            {
                localPipe = new VirtualCameraPipe();
                _ = localPipe.RunAsync(token);
                localRegistration = await Task.Run(VirtualCameraRegistration.Start, token);
            }
            using var localCameraLifetime = localRegistration;
            MediaFoundationCamera? capture = null;
            uint frameId = 0;
            var delay = TimeSpan.FromMilliseconds(1000d / selectedFps);
            try
            {
                while (!token.IsCancellationRequested)
                {
                    try
                    {
                        if (!useTestPattern && capture is null)
                            capture = await Task.Run(() => MediaFoundationCamera.Open(selectedDevice!, selectedFormat!), token);
                        var started = Stopwatch.GetTimestamp();
                        ++frameId;
                        using var frame = useTestPattern ? MakeTestPattern(frameId) : await Task.Run(() => capture!.ReadFrame(), token);
                        var jpeg = EncodeJpeg(frame, selectedQuality);
                        var target = peer.Current;
                        if (target is not null)
                        {
                            await SendFrameAsync(udp, target, frameId, jpeg, token);
                            sentFrames++;
                        }
                        ShowFrame(frame);
                        localPipe?.Post(frame);
                        if (target is null) SetStatus("수신 연결 대기 중 · 이 PC IP: " + LocalAddresses());
                        else UpdateStats("송신", sentFrames, jpeg.Length);
                        var elapsed = Stopwatch.GetElapsedTime(started);
                        if (elapsed < delay) await Task.Delay(delay - elapsed, token);
                    }
                    catch (Exception ex) when (!useTestPattern && ex is COMException or IOException)
                    {
                        capture?.Dispose(); capture = null;
                        SetStatus($"카메라 연결 끊김 · 2초 후 재시도 ({ex.Message})");
                        await Task.Delay(2000, token);
                    }
                }
            }
            finally { capture?.Dispose(); discoveryCts.Cancel(); await discoveryTask; }
        }
        catch (OperationCanceledException) { }
        catch (Exception ex)
        {
            Fail(!useTestPattern && ex is COMException
                ? $"카메라를 열거나 읽지 못했습니다. OBS와 동시 사용을 드라이버가 지원하지 않을 수 있습니다. Windows 카메라 권한과 OBS의 장치 사용 상태를 확인하세요. ({ex.Message})"
                : ex.Message);
        }
    }

    async Task RefreshCamerasAsync()
    {
        if (cts is not null) return;
        refreshCameras.Enabled = false;
        try
        {
            var devices = await Task.Run(MediaFoundationCamera.Enumerate);
            camera.Items.Clear();
            camera.Items.AddRange(devices.Cast<object>().ToArray());
            if (camera.Items.Count > 0) camera.SelectedIndex = 0;
            else SetStatus("연결된 카메라가 없습니다.");
        }
        catch (Exception ex) { SetStatus("카메라 열거 실패: " + ex.Message); }
        finally { refreshCameras.Enabled = true; }
    }

    async Task RefreshFormatsAsync()
    {
        cameraFormat.Items.Clear();
        if (camera.SelectedItem is not CameraDevice device) return;
        try
        {
            var formats = await Task.Run(() => MediaFoundationCamera.Formats(device));
            if (camera.SelectedItem is not CameraDevice current || current != device) return;
            cameraFormat.Items.AddRange(formats.Cast<object>().ToArray());
            var preferred = formats.Select((f, i) => (f, i)).FirstOrDefault(x => x.f.Width == 1920 && x.f.Height == 1080 && x.f.Fps == 30);
            if (formats.Count > 0) cameraFormat.SelectedIndex = preferred.f is null ? 0 : preferred.i;
        }
        catch (Exception ex) { SetStatus("카메라 형식 조회 실패: " + ex.Message); }
    }

    async Task RunReceiverAsync(CancellationToken token)
    {
        try
        {
            if (!IPAddress.TryParse(remoteIp.Text.Trim(), out var senderIp) || !IsPrivateLan(senderIp))
                throw new InvalidOperationException("송출컴의 사설 LAN IPv4 주소(10.x, 172.16~31.x, 192.168.x)를 입력하세요.");
            using var udp = new UdpClient(VideoPort);
            var helloTask = SendHelloAsync(udp, new IPEndPoint(senderIp, DiscoveryPort), token);
            var virtualPipe = new VirtualCameraPipe();
            _ = virtualPipe.RunAsync(token);
            VirtualCameraRegistration? virtualCamera = null;
            string virtualStatus;
            try
            {
                virtualCamera = await Task.Run(VirtualCameraRegistration.Start, token);
                virtualStatus = "CamBridge 가상 카메라 사용 가능";
            }
            catch (Exception ex)
            {
                virtualStatus = "가상 카메라 등록 실패: " + ex.Message;
            }
            using var cameraLifetime = virtualCamera;
            var frames = new Dictionary<uint, FrameParts>();
            SetStatus($"수신 대기 중 (UDP {VideoPort}) · {virtualStatus}");
            while (!token.IsCancellationRequested)
            {
                var result = await udp.ReceiveAsync(token);
                if (!result.RemoteEndPoint.Address.Equals(senderIp)) continue;
                if (!Packet.TryParse(result.Buffer, out var id, out var index, out var count, out var payload)) continue;
                foreach (var stale in frames.Where(x => x.Value.Age > TimeSpan.FromSeconds(1)).Select(x => x.Key).ToArray()) frames.Remove(stale);
                if (frames.Count >= 8 && !frames.ContainsKey(id)) continue;
                if (!frames.TryGetValue(id, out var parts)) frames[id] = parts = new FrameParts(count);
                if (parts.Count != count) { frames.Remove(id); continue; }
                if (!parts.Add(index, payload)) { frames.Remove(id); continue; }
                if (parts.Complete)
                {
                    var jpg = parts.Join(); frames.Remove(id);
                    try
                    {
                        using var stream = new MemoryStream(jpg, false);
                        using var decoded = Image.FromStream(stream);
                        using var frame = new Bitmap(decoded);
                        receivedFrames++; ShowFrame(frame); UpdateStats("수신", receivedFrames, jpg.Length);
                        virtualPipe.Post(frame);
                    }
                    catch (Exception ex) when (ex is ArgumentException or ExternalException)
                    { /* A damaged JPEG must not stop the receiver. */ }
                }
            }
            await helloTask;
        }
        catch (OperationCanceledException) { }
        catch (Exception ex) { Fail(ex.Message); }
    }

    static async Task SendHelloAsync(UdpClient udp, IPEndPoint sender, CancellationToken token)
    {
        var hello = "CamBridge receiver v1"u8.ToArray();
        try
        {
            while (!token.IsCancellationRequested)
            {
                await udp.SendAsync(hello, sender, token);
                await Task.Delay(2000, token);
            }
        }
        catch (OperationCanceledException) { }
    }

    static async Task ListenForReceiverAsync(UdpClient udp, DiscoveryPeer peer, CancellationToken token)
    {
        try
        {
            while (!token.IsCancellationRequested)
            {
                var result = await udp.ReceiveAsync(token);
                if (IsPrivateLan(result.RemoteEndPoint.Address) &&
                    result.Buffer.AsSpan().SequenceEqual("CamBridge receiver v1"u8))
                    peer.Update(new IPEndPoint(result.RemoteEndPoint.Address, VideoPort));
            }
        }
        catch (OperationCanceledException) { }
    }

    static string LocalAddresses()
    {
        var interfaces = NetworkInterface.GetAllNetworkInterfaces()
            .Where(x => x.OperationalStatus == OperationalStatus.Up &&
                (x.NetworkInterfaceType == NetworkInterfaceType.Ethernet || x.NetworkInterfaceType == NetworkInterfaceType.Wireless80211) &&
                !x.Description.Contains("virtual", StringComparison.OrdinalIgnoreCase) &&
                !x.Description.Contains("vpn", StringComparison.OrdinalIgnoreCase) &&
                !x.Description.Contains("tailscale", StringComparison.OrdinalIgnoreCase) &&
                x.GetIPProperties().GatewayAddresses.Any(g => g.Address.AddressFamily == AddressFamily.InterNetwork))
            .SelectMany(x => x.GetIPProperties().UnicastAddresses)
            .Where(x => x.Address.AddressFamily == AddressFamily.InterNetwork && IsPrivateLan(x.Address))
            .Select(x => x.Address.ToString()).Distinct().ToArray();
        return interfaces.Length == 0 ? "유선/Wi-Fi 사설 LAN IPv4 없음" : string.Join(", ", interfaces);
    }

    static bool IsPrivateLan(IPAddress address)
    {
        var bytes = address.GetAddressBytes();
        return bytes.Length == 4 && (bytes[0] == 10 ||
            bytes[0] == 172 && bytes[1] is >= 16 and <= 31 ||
            bytes[0] == 192 && bytes[1] == 168);
    }

    static async Task SendFrameAsync(UdpClient udp, IPEndPoint target, uint id, byte[] data, CancellationToken token)
    {
        // Stay below the usual Ethernet MTU so one lost IP fragment cannot discard a huge UDP datagram.
        const int chunkSize = 1200;
        if (data.Length > 8 * 1024 * 1024) throw new IOException("JPEG 프레임이 8MB 제한을 초과했습니다.");
        var count = (ushort)Math.Ceiling(data.Length / (double)chunkSize);
        for (ushort i = 0; i < count; i++)
        {
            var offset = i * chunkSize; var length = Math.Min(chunkSize, data.Length - offset);
            var packet = Packet.Build(id, i, count, data.AsSpan(offset, length));
            await udp.SendAsync(packet, target, token);
        }
    }

    static Bitmap MakeTestPattern(uint id)
    {
        var bitmap = new Bitmap(1280, 720, PixelFormat.Format24bppRgb);
        using var graphics = Graphics.FromImage(bitmap);
        graphics.Clear(Color.FromArgb(22, 28, 38));
        var x = (int)(id * 8 % 1100);
        using var accent = new SolidBrush(Color.FromArgb(250, 180, 35));
        graphics.FillRectangle(accent, x, 220, 180, 180);
        using var titleFont = new Font("Segoe UI", 40, FontStyle.Bold);
        using var clockFont = new Font("Consolas", 25);
        graphics.DrawString("CamBridge TEST", titleFont, Brushes.White, 55, 45);
        graphics.DrawString(DateTime.Now.ToString("yyyy-MM-dd HH:mm:ss.fff"), clockFont, Brushes.LightGray, 55, 635);
        return bitmap;
    }

    static byte[] EncodeJpeg(Bitmap bitmap, long quality)
    {
        using var stream = new MemoryStream();
        var codec = ImageCodecInfo.GetImageEncoders().First(x => x.FormatID == ImageFormat.Jpeg.Guid);
        using var parameters = new EncoderParameters(1);
        parameters.Param[0] = new EncoderParameter(System.Drawing.Imaging.Encoder.Quality, quality);
        bitmap.Save(stream, codec, parameters);
        return stream.ToArray();
    }

    void ShowFrame(Bitmap frame)
    {
        var copy = new Bitmap(frame);
        if (IsDisposed) { copy.Dispose(); return; }
        BeginInvoke(() => { var old = preview.Image; preview.Image = copy; old?.Dispose(); });
    }

    void UpdateStats(string verb, long frames, int bytes)
    {
        if ((DateTime.UtcNow - statsAt).TotalMilliseconds < 500) return;
        statsAt = DateTime.UtcNow;
        SetStatus($"{verb} 중 · 프레임 {frames:N0} · 최근 {(bytes / 1024d):N0} KB");
    }

    void SetStatus(string text) { if (!IsDisposed) BeginInvoke(() => status.Text = text); }
    void Fail(string text)
    {
        if (IsDisposed) return;
        BeginInvoke(() =>
        {
            cts?.Cancel(); cts = null; toggle.Text = "시작"; LockSettings(false);
            status.Text = "오류: " + text;
            tray.ShowBalloonTip(4000, "CamBridge 오류", text, ToolTipIcon.Error);
        });
    }

    void ShowFromTray() { Show(); WindowState = FormWindowState.Normal; Activate(); }

    static string StartupKey => @"Software\Microsoft\Windows\CurrentVersion\Run";
    static bool IsStartupEnabled()
    {
        using var key = Registry.CurrentUser.OpenSubKey(StartupKey);
        return key?.GetValue("CamBridge") is not null;
    }
    static void SetStartup(bool enabled)
    {
        using var key = Registry.CurrentUser.CreateSubKey(StartupKey);
        if (enabled) key.SetValue("CamBridge", $"\"{Environment.ProcessPath}\""); else key.DeleteValue("CamBridge", false);
    }
}

internal sealed class DiscoveryPeer
{
    readonly object gate = new();
    IPEndPoint? endpoint;
    DateTime lastSeen;

    public void Update(IPEndPoint value)
    {
        lock (gate) { endpoint = value; lastSeen = DateTime.UtcNow; }
    }

    public IPEndPoint? Current
    {
        get { lock (gate) return DateTime.UtcNow - lastSeen < TimeSpan.FromSeconds(6) ? endpoint : null; }
    }
}

internal static class Packet
{
    static readonly byte[] Magic = "CBR1"u8.ToArray();
    public static byte[] Build(uint id, ushort index, ushort count, ReadOnlySpan<byte> payload)
    {
        var result = new byte[12 + payload.Length];
        Magic.CopyTo(result, 0);
        BitConverter.GetBytes(id).CopyTo(result, 4);
        BitConverter.GetBytes(index).CopyTo(result, 8);
        BitConverter.GetBytes(count).CopyTo(result, 10);
        payload.CopyTo(result.AsSpan(12));
        return result;
    }
    public static bool TryParse(byte[] packet, out uint id, out ushort index, out ushort count, out byte[] payload)
    {
        id = 0; index = count = 0; payload = [];
        if (packet.Length < 13 || !packet.AsSpan(0, 4).SequenceEqual(Magic)) return false;
        id = BitConverter.ToUInt32(packet, 4); index = BitConverter.ToUInt16(packet, 8); count = BitConverter.ToUInt16(packet, 10);
        if (count == 0 || count > 8192 || index >= count || packet.Length > 60_012) return false;
        payload = packet[12..]; return true;
    }
}

internal sealed class FrameParts
{
    readonly byte[][] chunks;
    readonly DateTime created = DateTime.UtcNow;
    int received;
    int bytes;
    public FrameParts(ushort count) => chunks = new byte[count][];
    public TimeSpan Age => DateTime.UtcNow - created;
    public int Count => chunks.Length;
    public bool Complete => received == chunks.Length;
    public bool Add(ushort index, byte[] data)
    {
        if (chunks[index] is not null) return true;
        if (bytes + data.Length > 8 * 1024 * 1024) return false;
        chunks[index] = data; received++; bytes += data.Length; return true;
    }
    public byte[] Join()
    {
        var result = new byte[chunks.Sum(x => x.Length)]; var offset = 0;
        foreach (var chunk in chunks) { Buffer.BlockCopy(chunk, 0, result, offset, chunk.Length); offset += chunk.Length; }
        return result;
    }
}
