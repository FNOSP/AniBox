// SetupUi.cs — AniBox 自绘安装器 UI 进程(WPF + WebView2)
// 架构: NSIS stub 启动本进程 → HTML/CSS 四页界面 → 选定模式/路径后以 /S 静默参数
// 重新拉起安装器本体(Tauri NSIS 安装逻辑零改动), 本进程轮询 INSTDIR 字节数换算真实进度。
using System;
using System.Collections.Generic;
using System.IO;
using System.IO.Compression;
using System.Runtime.InteropServices;
using System.Threading;
using System.Windows;
using System.Windows.Threading;
using Microsoft.Web.WebView2.Core;
using Microsoft.Web.WebView2.Wpf;

internal static class Dwm {
    [DllImport("dwmapi.dll")]
    public static extern int DwmSetWindowAttribute(IntPtr hwnd, int attr, ref int value, int size);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongW")]
    public static extern int GetWindowLong(IntPtr hwnd, int index);
    [DllImport("user32.dll", EntryPoint = "SetWindowLongW")]
    public static extern int SetWindowLong(IntPtr hwnd, int index, int value);
    [DllImport("user32.dll")]
    public static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
}

// 读取子进程累计写入字节数(GetProcessIoCounters)。
// 为什么用它而不是轮询 INSTDIR 体积: 覆盖/升级安装时目标目录里旧文件仍在,
// 目录体积一开始就等于"已装完", 换算出的百分比会瞬间顶到上限(94%)并卡住。
// 进程 I/O 计数器统计的是"本次真正写出的字节", 全新安装与覆盖安装都准确。
internal static class ProcIo {
    [StructLayout(LayoutKind.Sequential)]
    struct IO_COUNTERS {
        public ulong ReadOperationCount;
        public ulong WriteOperationCount;
        public ulong ReadTransferCount;
        public ulong WriteTransferCount;
        public ulong OtherTransferCount;
    }
    const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetProcessIoCounters(IntPtr h, out IO_COUNTERS c);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CloseHandle(IntPtr h);

    // 成功返回累计写入字节数(>=0); 失败(权限/进程已退出等)返回 -1, 调用方回退到目录体积。
    public static long WrittenBytes(int pid) {
        if (pid <= 0) return -1;
        IntPtr h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if (h == IntPtr.Zero) return -1;
        try {
            IO_COUNTERS c;
            if (!GetProcessIoCounters(h, out c)) return -1;
            return (long)c.WriteTransferCount;
        } finally { CloseHandle(h); }
    }
}

namespace AniBoxSetupUi {
    static class Args {
        public static string Get(string[] a, string name, string def = null) {
            for (int i = 0; i < a.Length; i++) {
                var k = a[i];
                if (k.StartsWith("--" + name + "=", StringComparison.OrdinalIgnoreCase)) return k.Substring(name.Length + 3);
                if (k.Equals("--" + name, StringComparison.OrdinalIgnoreCase) && i + 1 < a.Length) return a[++i];
            }
            return def;
        }
        // 裸开关（--uninstall）不能走 Get: Get 会把下一个参数当成它的值
        public static bool Has(string[] a, string name) {
            foreach (var k in a) {
                if (k.Equals("--" + name, StringComparison.OrdinalIgnoreCase)) return true;
                if (k.StartsWith("--" + name + "=", StringComparison.OrdinalIgnoreCase)) return true;
            }
            return false;
        }
    }

    // 极简扁平 JSON(本项目 web 消息全是扁平 string/number), 不引第三方依赖
    static class MiniJson {
        public static Dictionary<string, string> Parse(string s) {
            var d = new Dictionary<string, string>();
            if (string.IsNullOrEmpty(s)) return d;
            int i = 0, n = s.Length;
            Func<int, int> skipWs = (p) => { while (p < n && (s[p] == ' ' || s[p] == '\t' || s[p] == '\r' || s[p] == '\n')) p++; return p; };
            i = skipWs(i);
            if (i >= n || s[i] != '{') return d;
            i++;
            while (i < n) {
                i = skipWs(i);
                if (i < n && s[i] == '}') break;
                if (i < n && s[i] == ',') { i++; continue; }
                if (i >= n || s[i] != '"') break;
                int q = s.IndexOf('"', i + 1);
                if (q < 0) break;
                var key = Unescape(s.Substring(i + 1, q - i - 1));
                i = skipWs(q + 1);
                if (i >= n || s[i] != ':') break;
                i = skipWs(i + 1);
                if (i < n && s[i] == '"') {
                    var sb = new System.Text.StringBuilder();
                    i++;
                    while (i < n && s[i] != '"') {
                        if (s[i] == '\\' && i + 1 < n) {
                            char c = s[i + 1];
                            if (c == 'n') sb.Append('\n');
                            else if (c == 't') sb.Append('\t');
                            else if (c == 'r') sb.Append('\r');
                            else if (c == 'u' && i + 5 < n) {
                                sb.Append((char)Convert.ToInt32(s.Substring(i + 2, 4), 16));
                                i += 4;
                            } else sb.Append(c);
                            i += 2;
                        } else { sb.Append(s[i]); i++; }
                    }
                    i++;
                    d[key] = sb.ToString();
                } else {
                    int st = i;
                    while (i < n && s[i] != ',' && s[i] != '}') i++;
                    d[key] = s.Substring(st, i - st).Trim();
                }
            }
            return d;
        }
        static string Unescape(string s) {
            return s.Replace("\\\"", "\"").Replace("\\\\", "\\").Replace("\\n", "\n").Replace("\\t", "\t");
        }
        public static string Esc(string s) {
            if (s == null) return "";
            var sb = new System.Text.StringBuilder();
            foreach (char c in s) {
                switch (c) {
                    case '"': sb.Append("\\\""); break;
                    case '\\': sb.Append("\\\\"); break;
                    case '\n': sb.Append("\\n"); break;
                    case '\r': sb.Append("\\r"); break;
                    case '\t': sb.Append("\\t"); break;
                    default:
                        if (c < 32) sb.Append("\\u").Append(((int)c).ToString("x4"));
                        else sb.Append(c);
                        break;
                }
            }
            return sb.ToString();
        }
        public static string Obj(params string[] kv) {
            var sb = new System.Text.StringBuilder("{");
            for (int i = 0; i + 1 < kv.Length; i += 2) {
                if (i > 0) sb.Append(',');
                sb.Append('"').Append(kv[i]).Append("\":");
                long numeric;
                if (kv[i + 1] == "true" || kv[i + 1] == "false" || long.TryParse(kv[i + 1], out numeric))
                    sb.Append(kv[i + 1]);
                else
                    sb.Append('"').Append(Esc(kv[i + 1])).Append('"');
            }
            return sb.Append('}').ToString();
        }
    }

    public class App : Application {
        Window _win;
        WebView2 _wv;
        string _setupExe, _wwwDir, _wwwBase, _workDir, _instDir = "", _exeName = "anibox.exe", _version = "";
        string _installedVersion = "", _existingDir = "", _existingMode = "", _releaseDate = "";
        int _legacyCount;
        bool _sameVersionInstalled;
        string _legacyVersions = "";
        long _totalSize;
        string[] _launchArgs = new string[0];
        bool _dev, _installing, _finished, _launched;
        System.Diagnostics.Process _child;
        DispatcherTimer _poll;
        // 进度采样: 本次已写字节(单调递增) / 安装前目标目录基线 / 上次百分比(防回退)
        long _baselineSize, _writtenBytes;
        // 开始写文件(extract.flag)那一刻的进程写入基线, 用来剔除解压前的杂项写入
        // (例如 WebView2 引导程序下载/解压), -1 表示尚未采样
        long _ioBaseline = -1;
        int _lastPct;
        string _defaultUserPath, _defaultAllPath;

        // ── 卸载模式 ──
        bool _uninstallMode, _uninstalling, _uninstalled, _autoUninstall;
        string _uninstallExe = "", _uninstVersion = "", _dataPathText = "";
        long _installSize, _dataSize;
        bool _purgeData;

        [STAThread]
        static void Main(string[] args) {
            bool created;
            // 安装 UI 与卸载 UI 用不同互斥体: 二者可能同时存在(例如升级前手动卸载)
            bool isUninstall = false;
            foreach (var a in args)
                if (a.Equals("--uninstall", StringComparison.OrdinalIgnoreCase)) { isUninstall = true; break; }
            var mutex = new Mutex(true, isUninstall ? "AniBox.UninstallUI" : "AniBox.SetupUI", out created);
            if (!created) return;
            Log("launch: " + string.Join(" ", args));
            AppDomain.CurrentDomain.UnhandledException += (s, e) => Log("Unhandled: " + e.ExceptionObject);
            System.Windows.Forms.Application.SetUnhandledExceptionMode(System.Windows.Forms.UnhandledExceptionMode.CatchException);
            System.Windows.Forms.Application.ThreadException += (s, e) => Log("ThreadException: " + e.Exception);
            var app = new App();
            app.DispatcherUnhandledException += (s, e) => { Log("Dispatcher: " + e.Exception); e.Handled = true; };
            try { app.Run(); } catch (Exception ex) { Log("Run: " + ex); } finally { mutex.ReleaseMutex(); }
            Log("exit");
        }

        static void Log(object msg) {
            try {
                File.AppendAllText(
                    Path.Combine(Path.GetTempPath(), "AniBoxSetupUi.log"),
                    DateTime.Now.ToString("HH:mm:ss.fff ") + msg + Environment.NewLine);
            } catch { }
        }

        protected override void OnStartup(StartupEventArgs e) {
            _launchArgs = e.Args;
            _dev = Args.Has(e.Args, "dev");
            _uninstallMode = Args.Has(e.Args, "uninstall");
            _autoUninstall = Args.Has(e.Args, "auto-uninstall");
            _setupExe = Args.Get(e.Args, "setup", "");
            _wwwDir = Args.Get(e.Args, "www", "");
            _workDir = Path.Combine(Path.GetTempPath(), "anibox-setup-" + Guid.NewGuid().ToString("N").Substring(0, 8));
            Directory.CreateDirectory(_workDir);

            _defaultUserPath = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "AniBox");
            _defaultAllPath = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "AniBox");

            if (string.IsNullOrEmpty(_wwwDir)) _wwwDir = AppDomain.CurrentDomain.BaseDirectory;
            _wwwBase = _wwwDir;   // 资材基目录(totalsize/app-meta 所在, zip 解压后 _wwwDir 会切走)

            // 打包态: www.zip 就地解压(开发态直接用 www 目录)
            string zip = Path.Combine(_wwwDir, "www.zip");
            if (!File.Exists(zip)) zip = Path.Combine(_wwwDir, "ui.zip");   // 兼容旧命名
            if (File.Exists(zip)) {
                var ext = Path.Combine(_workDir, "www");
                ZipFile.ExtractToDirectory(zip, ext);
                _wwwDir = ext;
                Log("www extracted -> " + ext);
            }

            LoadMeta();

            if (_uninstallMode) {
                _uninstallExe = Args.Get(e.Args, "exe", "");
                _instDir = (Args.Get(e.Args, "dir", "") ?? "").TrimEnd('\\');
                _uninstVersion = Args.Get(e.Args, "version", "");
                if (!string.IsNullOrEmpty(_uninstVersion)) _version = _uninstVersion;
                if (string.IsNullOrEmpty(_instDir)) {
                    DetectExistingInstall();
                    _instDir = string.IsNullOrEmpty(_existingDir) ? _defaultUserPath : _existingDir;
                }
                _purgeData = Args.Get(e.Args, "purge", "0") == "1";
                _installSize = DirSizeSafe(_instDir);
                _dataSize = DataSize();
                _dataPathText = string.Join("\n", DataDirs());
                Log("uninstall: exe=" + _uninstallExe + " dir=" + _instDir + " size=" + _installSize +
                    " data=" + _dataSize + " purge=" + (_purgeData ? "1" : "0"));
            } else {
                DetectExistingInstall();
                _instDir = string.IsNullOrEmpty(_existingDir) ? _defaultUserPath : _existingDir;
            }

            // ── WebView2 Runtime 前置检查（必须在创建 WebView2 之前）──
            // 本 UI 由 WebView2 渲染；运行时缺失时窗口是空白的，连提示都显示不出来。
            // 因此这里用原生 MessageBox 说明「缺少 WebView2 将无法运行」，得到用户同意后
            // 在线下载安装，安装成功重新检测，仍缺失则中止（安装）或回退原生卸载界面（卸载）。
            if (!_dev && !Args.Has(_launchArgs, "skip-webview2-check")) {
                if (!WebView2Runtime.EnsureInteractive(_uninstallMode)) {
                    Log("webview2: aborted by user or install failed");
                    if (_uninstallMode) FallbackNativeUninstall();
                    Shutdown(1);
                    return;
                }
            }

            _win = new Window {
                Title = _uninstallMode ? "AniBox 卸载" : "AniBox 安装",
                Width = 780,
                Height = 520,
                WindowStartupLocation = WindowStartupLocation.CenterScreen,
                WindowStyle = WindowStyle.None,
                ResizeMode = ResizeMode.NoResize,
                ShowInTaskbar = true,
                Background = System.Windows.Media.Brushes.White,
                Icon = TryLoadIcon()
            };
            _wv = new WebView2 { DefaultBackgroundColor = System.Drawing.Color.Transparent };
            _win.Content = _wv;
            _win.Loaded += async (s, ev) => {
                try {
                    var env = await CoreWebView2Environment.CreateAsync(
                        null, Path.Combine(_workDir, "webview2"), new CoreWebView2EnvironmentOptions());
                    await _wv.EnsureCoreWebView2Async(env);
                    var c = _wv.CoreWebView2;
                    c.WebMessageReceived += OnWebMessage;
                    c.SetVirtualHostNameToFolderMapping("installer.local", _wwwDir, CoreWebView2HostResourceAccessKind.Allow);
                    c.NavigationCompleted += (s2, e2) => Log("nav ok=" + e2.IsSuccess + " err=" + e2.WebErrorStatus);
                    string shotsDir = Args.Get(_launchArgs, "shots", "");
                    bool tour = !string.IsNullOrEmpty(shotsDir) && !_uninstallMode;
                    c.Navigate(tour ? "https://installer.local/index.html?tour=1" : "https://installer.local/index.html");
                    if (!string.IsNullOrEmpty(shotsDir)) ScheduleShots(shotsDir);
                } catch (Exception ex) {
                    MessageBox.Show("WebView2 运行时初始化失败: " + ex.Message +
                        "\n\n请安装 Microsoft Edge WebView2 运行时后重试。", "AniBox 安装",
                        MessageBoxButton.OK, MessageBoxImage.Warning);
                    Shutdown(1);
                }
            };
            MainWindow = _win;
            // Win11 圆角三件套: 光杆 WS_POPUP 无边框窗 DWM 不圆化(hr=0 也不画),
            // ① WindowChrome 接管 WM_NCCALCSIZE/命中(屏蔽 THICKFRAME 的缩放与边框内缩),
            // ② 补回 WS_THICKFRAME 样式(Electron 同款做法),
            // ③ DWMWCP_ROUND(33=2)。Win10 无该属性, 方角静默兜底。
            var chrome = new System.Windows.Shell.WindowChrome {
                CaptionHeight = 0,
                ResizeBorderThickness = new Thickness(0),
                GlassFrameThickness = new Thickness(0),
                CornerRadius = new CornerRadius(0),
                UseAeroCaptionButtons = false
            };
            System.Windows.Shell.WindowChrome.SetWindowChrome(_win, chrome);
            _win.SourceInitialized += (s, ev) => {
                try {
                    var hwnd = new System.Windows.Interop.WindowInteropHelper(_win).Handle;
                    EnsureRound(hwnd);
                    var src = System.Windows.Interop.HwndSource.FromHwnd(hwnd);
                    if (src != null) src.AddHook((IntPtr h2, int msg2, IntPtr wp2, IntPtr lp2, ref bool handled2) => {
                        if (msg2 == 0x0006 || msg2 == 0x0005) {   // WM_ACTIVATE / WM_SIZE
                            Dispatcher.BeginInvoke(new Action(() => EnsureRound(hwnd)),
                                DispatcherPriority.Background);
                        }
                        return IntPtr.Zero;
                    });
                } catch (Exception ex) { Log("corner pref: " + ex.Message); }
            };
            _win.Show();
        }

        // WebView2 缺失且用户拒绝安装时，卸载仍要能进行：
        // 回退到 NSIS 原生卸载界面（不依赖 WebView2）。`/_UNINST=` 空值让 un.onInit
        // 跳过「解压自绘 UI」，直接进入原生确认/进度页。
        void FallbackNativeUninstall() {
            try {
                string exe = _uninstallExe;
                if (string.IsNullOrEmpty(exe) || !File.Exists(exe)) {
                    string guess = Path.Combine(_instDir ?? "", "uninstall.exe");
                    if (File.Exists(guess)) exe = guess;
                }
                if (string.IsNullOrEmpty(exe) || !File.Exists(exe)) {
                    MessageBox.Show("找不到卸载程序，无法继续卸载。请重新运行安装包进行覆盖安装后再卸载。",
                        "AniBox 卸载", MessageBoxButton.OK, MessageBoxImage.Error);
                    return;
                }
                Log("uninstall fallback -> native UI: " + exe);
                System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo {
                    FileName = exe,
                    Arguments = "/_UNINST=",
                    UseShellExecute = true,
                    WorkingDirectory = Path.GetTempPath()
                });
            } catch (Exception ex) {
                Log("uninstall fallback failed: " + ex.Message);
                MessageBox.Show("无法启动原生卸载界面：" + ex.Message,
                    "AniBox 卸载", MessageBoxButton.OK, MessageBoxImage.Error);
            }
        }

        void EnsureRound(IntPtr hwnd) {
            try {
                const int GWL_STYLE = -16, WS_THICKFRAME = 0x00040000;
                if ((Dwm.GetWindowLong(hwnd, GWL_STYLE) & WS_THICKFRAME) == 0) {
                    Dwm.SetWindowLong(hwnd, GWL_STYLE, Dwm.GetWindowLong(hwnd, GWL_STYLE) | WS_THICKFRAME);
                    Dwm.SetWindowPos(hwnd, IntPtr.Zero, 0, 0, 0, 0, 0x27);   // NOMOVE|NOSIZE|NOZORDER|FRAMECHANGED
                }
                int pref = 2;   // DWMWCP_ROUND
                Dwm.DwmSetWindowAttribute(hwnd, 33, ref pref, 4);
            } catch (Exception ex) { Log("ensure round: " + ex.Message); }
        }

        System.Windows.Media.ImageSource TryLoadIcon() {
            try {
                var p = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "icon.ico");
                if (!File.Exists(p)) p = Path.Combine(Path.GetDirectoryName(_setupExe) ?? "", "icon.ico");
                if (!File.Exists(p)) return null;
                var dec = new System.Windows.Media.Imaging.IconBitmapDecoder(
                    new Uri(p), System.Windows.Media.Imaging.BitmapCreateOptions.None,
                    System.Windows.Media.Imaging.BitmapCacheOption.OnLoad);
                System.Windows.Media.Imaging.BitmapFrame best = null;
                foreach (var f in dec.Frames)
                    if (best == null || f.PixelWidth > best.PixelWidth) best = f;
                return best;
            } catch (Exception ex) { Log("icon: " + ex.Message); return null; }
        }

        // --shots=<dir>: 巡演定时截图(WebView2 CapturePreview, 不截屏幕), 含协议弹层
        void ScheduleShots(string dir) {
            Directory.CreateDirectory(dir);
            var beats = _uninstallMode
                ? new[] {
                    new { delay = 700, name = "u1-confirm" },
                    new { delay = 2000, name = "u2-progress" },
                    new { delay = 7000, name = "u3-finish" },
                }
                : new[] {
                    new { delay = 800, name = "p1-welcome" },
                    new { delay = 1600, name = "p1b-license" },
                    new { delay = 3400, name = "p2-options" },
                    new { delay = 5400, name = "p3-progress" },
                    new { delay = 11500, name = "p4-finish" },
                };
            foreach (var b in beats) {
                var t = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(b.delay) };
                t.Tick += async (s, e) => {
                    t.Stop();
                    try {
                        using (var fs = File.Create(Path.Combine(dir, b.name + ".png")))
                            await _wv.CoreWebView2.CapturePreviewAsync(CoreWebView2CapturePreviewImageFormat.Png, fs);
                        Log("shot " + b.name);
                    } catch (Exception ex) { Log("shot fail " + b.name + ": " + ex.Message); }
                    if (b.name == (_uninstallMode ? "u3-finish" : "p4-finish")) Shutdown(0);
                };
                t.Start();
            }
        }

        void LoadMeta() {
            try {
                var j = MiniJson.Parse(File.ReadAllText(Path.Combine(_wwwBase, "app-meta.json")));
                _version = j.ContainsKey("version") ? j["version"] : "";
                _exeName = j.ContainsKey("exeName") ? j["exeName"] : _exeName;
                if (j.ContainsKey("releaseDate")) _releaseDate = j["releaseDate"];
            } catch { }
            try {
                _totalSize = long.Parse(File.ReadAllText(Path.Combine(_wwwBase, "totalsize.txt")).Trim());
            } catch { _totalSize = 0; }
            Log("meta: version=" + _version + " exe=" + _exeName + " total=" + _totalSize + " release=" + _releaseDate);
        }

        // 检测本机已装旧版（AniBox / Fntv-Plus）。
        //
        // 关键修复：旧实现只匹配 DisplayName 里的 "AniBox"，完全漏掉了
        // Electron 版 "Fntv-Plus 3.9.0"，所以升级时一直显示「首次安装」。
        // 现由 InstallDetect 统一处理：
        //   1. 注册表 HKCU / HKCU32 / HKLM64 / HKLM32 的 Uninstall 项，
        //      DisplayName 含 AniBox 或 Fntv-Plus；
        //   2. 文件系统兜底（%LOCALAPPDATA%\Programs\Fntv-Plus 等），
        //      与 installer-hooks.nsh 实际会卸载的目录一致；
        //   3. 版本等于当前安装包版本的项归为「覆盖重装」，不算旧版。
        void DetectExistingInstall() {
            try {
                var r = InstallDetect.Detect(_version, _exeName, _defaultUserPath, _defaultAllPath);
                foreach (var d in r.All)
                    Log("detect hit: src=" + d.Source + " key=" + d.KeyPath + " name=" + d.DisplayName +
                        " ver=" + d.Version + " dir=" + d.Dir + " kind=" + d.Kind);

                _legacyCount = r.Legacy.Count;
                _sameVersionInstalled = r.SameVersion.Count > 0;

                // 已安装版本 = 所有命中项中的最高版本（语义比较）。
                // 修复：旧逻辑只在有旧版时才取 Primary.Version，于是当机器上同时存在
                // 4.0.0 与某条 3.9.0 残留时，会显示成「升级安装 v3.9.0 → v4.0.0」。
                DetectedInstall highestItem = null;
                foreach (var d in r.All) {
                    if (string.IsNullOrEmpty(d.Version)) continue;
                    if (highestItem == null ||
                        InstallDetect.CompareVersion(d.Version, highestItem.Version) > 0)
                        highestItem = d;
                }
                if (highestItem != null) _installedVersion = highestItem.Version;

                if (r.SameVersion.Count > 0) {
                    // 已装同版本：以新装项为准（覆盖修复），旧版残留仅作提示。
                    var same = r.SameVersion[0];
                    if (string.IsNullOrEmpty(_installedVersion)) _installedVersion = same.Version;
                    _existingDir = same.Dir;
                    _existingMode = ModeForInstall(same);
                    if (_legacyCount > 0) _legacyVersions = JoinLegacyNames(r.Legacy);
                } else if (_legacyCount > 0) {
                    // 多个旧版：以版本最高者为主显示，全部列出供 UI 说明「一并清理」。
                    _legacyVersions = JoinLegacyNames(r.Legacy);
                    _installedVersion = r.Primary.Version;
                    _existingMode = ModeForInstall(r.Primary);

                    // Electron Fntv-Plus 不装进它的旧目录：hooks 会先卸载它，
                    // 新 AniBox 落到默认目录（否则会把 anibox.exe 混进 Electron 目录）。
                    if (r.Primary.Kind == "tauri" && !string.IsNullOrEmpty(r.Primary.Dir)) {
                        _existingDir = r.Primary.Dir;
                    } else {
                        _existingDir = "";
                    }
                }
            } catch (Exception ex) {
                Log("detect: " + ex);
            }
            Log("detect result: installed=" + _installedVersion + " legacyCount=" + _legacyCount +
                " sameVersion=" + _sameVersionInstalled + " legacyVersions=" + _legacyVersions +
                " dir=" + _existingDir + " mode=" + _existingMode);
        }

        static string JoinLegacyNames(System.Collections.Generic.List<DetectedInstall> legacy) {
            var names = new List<string>();
            foreach (var d in legacy) {
                string label = string.IsNullOrEmpty(d.DisplayName) ? ("v" + d.Version) : d.DisplayName;
                if (!names.Contains(label)) names.Add(label);
            }
            return string.Join("、", names.ToArray());
        }

        string ModeForInstall(DetectedInstall d) {
            var dir = d.Dir ?? "";
            var pf = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles);
            var pfx86 = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86);
            if (dir.Length > 0) {
                if ((pf.Length > 0 && dir.StartsWith(pf, StringComparison.OrdinalIgnoreCase))
                    || (pfx86.Length > 0 && dir.StartsWith(pfx86, StringComparison.OrdinalIgnoreCase)))
                    return "all";
                var lad = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
                if (lad.Length > 0 && dir.StartsWith(lad, StringComparison.OrdinalIgnoreCase)) return "user";
            }
            if (d.Root.StartsWith("HKLM", StringComparison.OrdinalIgnoreCase)) return "all";
            if (d.Root.StartsWith("HKCU", StringComparison.OrdinalIgnoreCase)) return "user";
            // 文件系统兜底（Electron 旧版）默认按当前用户安装，避免无必要的 UAC。
            return "user";
        }

        long TotalSize() {
            return _totalSize;
        }

        void OnWebMessage(object s, CoreWebView2WebMessageReceivedEventArgs e) {
            var m = MiniJson.Parse(e.TryGetWebMessageAsString());
            string type;
            if (!m.TryGetValue("type", out type)) return;
            Log("web: " + type + (m.ContainsKey("mode") ? " mode=" + m["mode"] : "") + (m.ContainsKey("path") ? " path=" + m["path"] : "")
                + (m.ContainsKey("x") ? " at=" + m["x"] + "," + m["y"] + " t=" + (m.ContainsKey("t") ? m["t"] : "") : ""));
            switch (type) {
                case "ready":
                    if (_uninstallMode) {
                        Post(MiniJson.Obj("type", "meta", "mode", "uninstall",
                            "version", _version, "installed", _version,
                            "path", _instDir,
                            "installSize", _installSize.ToString(),
                            "dataSize", _dataSize.ToString(),
                            "dataPath", _dataPathText,
                            "purgeDefault", _purgeData ? "1" : "0"));
                        // --auto-uninstall: E2E 自动化(配合 --purge/--shots/--auto-exit)
                        if (_autoUninstall) Post(MiniJson.Obj("type", "autoUninstall", "purge", _purgeData ? "1" : "0"));
                        break;
                    }
                    Post(MiniJson.Obj("type", "meta", "version", _version, "exeName", _exeName,
                        "installed", _installedVersion,
                        "legacyCount", _legacyCount.ToString(),
                        "legacyVersions", _legacyVersions,
                        "sameVersion", _sameVersionInstalled ? "1" : "0",
                        "mode", string.IsNullOrEmpty(_existingMode) ? "user" : _existingMode,
                        "path", string.IsNullOrEmpty(_existingDir) ? _defaultUserPath : _existingDir,
                        "releaseDate", _releaseDate));
                    // --auto <mode>: E2E 自动化(配合 --auto-path/--auto-exit), 无人值守真机验证
                    string autoMode = Args.Get(_launchArgs, "auto", "");
                    if (autoMode == "user" || autoMode == "all") {
                        string autoPath = Args.Get(_launchArgs, "auto-path", "");
                        if (!string.IsNullOrEmpty(autoPath)) _instDir = autoPath;
                        Post(MiniJson.Obj("type", "folder", "path", _instDir));
                        StartInstall(autoMode);
                    }
                    break;
                case "modeChanged": {
                    string mode = m.ContainsKey("mode") ? m["mode"] : "user";
                    string cur = _instDir;
                    string want = mode == "all" ? _defaultAllPath : _defaultUserPath;
                    // 路径还停留在另一模式的默认值时才跟随切换, 用户自选过则保留
                    if (cur.Equals(mode == "all" ? _defaultUserPath : _defaultAllPath, StringComparison.OrdinalIgnoreCase) || string.IsNullOrEmpty(cur)) {
                        _instDir = want;
                        Post(MiniJson.Obj("type", "folder", "path", want));
                    }
                    break;
                }
                case "pickFolder": {
                    string cur = _instDir;
                    var th = new Thread(() => {
                        string picked = null, warn = null;
                        var dlg = new System.Windows.Forms.FolderBrowserDialog {
                            Description = "选择安装文件夹",
                            ShowNewFolderButton = true
                        };
                        if (Directory.Exists(cur)) dlg.SelectedPath = cur;
                        if (dlg.ShowDialog() == System.Windows.Forms.DialogResult.OK) {
                            picked = dlg.SelectedPath;
                            bool nonAscii = false;
                            foreach (char ch in picked) if (ch > 127) { nonAscii = true; break; }
                            if (nonAscii) warn = "该路径包含中文或非英文字符, 可能导致内置代理服务或外部播放器无法启动。";
                        }
                        Dispatcher.BeginInvoke(new Action(() => {
                            if (picked != null) { _instDir = picked; Post(MiniJson.Obj("type", "folder", "path", picked, "warn", warn ?? "")); }
                        }));
                    });
                    th.SetApartmentState(ApartmentState.STA);
                    th.Start();
                    break;
                }
                case "install": {
                    string mode = m.ContainsKey("mode") ? m["mode"] : "user";
                    string path = m.ContainsKey("path") ? m["path"] : "";
                    if (!string.IsNullOrEmpty(path)) _instDir = path;
                    StartInstall(mode);
                    break;
                }
                case "uninstall":
                    if (_uninstalling || _uninstalled) break;
                    _purgeData = m.ContainsKey("purge") && (m["purge"] == "1" || m["purge"] == "true");
                    StartUninstall();
                    break;
                case "reinstall":
                    // 卸载完成后重新安装: 打开官网下载页(卸载器本身不携带安装包)
                    try {
                        System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo {
                            FileName = "https://690075.xyz", UseShellExecute = true
                        });
                    } catch (Exception ex) { Log("reinstall open: " + ex.Message); }
                    break;
                case "launch":
                    // 修复: 旧实现只 Post("close") 给网页层, 而 app.js 没有处理 close,
                    // 于是向导永远不关, 应用窗口被压在向导后面 → 用户以为"点立即体验没反应"。
                    // 现在: 启动成功后由 C# 直接关闭向导(应用已在独立进程里运行, 关向导不影响它)。
                    if (!_finished) break;
                    if (!_launched) {
                        // 刚写盘的新 exe 可能被杀软短暂锁定 → 最多重试 3 次, 间隔 600ms
                        string exePath = Path.Combine(_instDir, _exeName);
                        for (int attempt = 1; attempt <= 3 && !_launched; attempt++) {
                            try {
                                System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo {
                                    FileName = exePath,
                                    WorkingDirectory = _instDir,
                                    UseShellExecute = true
                                });
                                _launched = true;
                            } catch (Exception ex) {
                                Log("launch attempt " + attempt + " failed: " + ex.Message);
                                if (attempt == 3) {
                                    Post(MiniJson.Obj("type", "error", "msg", "启动应用失败: " + ex.Message));
                                    return;   // 启动失败时保留窗口, 让用户看到提示
                                }
                                Thread.Sleep(600);
                            }
                        }
                    }
                    if (_launched) Shutdown(0);
                    break;
                case "minimize":
                    _win.WindowState = WindowState.Minimized;
                    break;
                case "close":
                    if ((_installing && !_finished) || (_uninstalling && !_uninstalled)) return;
                    Shutdown(0);
                    break;
                case "drag":
                    try { _win.DragMove(); } catch { }
                    break;
            }
        }

        void Post(string json) {
            if (_wv != null && _wv.CoreWebView2 != null) _wv.CoreWebView2.PostWebMessageAsJson(json);
        }

        void StartInstall(string mode) {
            _installing = true;
            // 记录安装前目录体积 + 清零进度状态(覆盖安装时目录已有旧文件, 不能当成本次进度)
            _baselineSize = DirSizeSafe(_instDir);
            _writtenBytes = 0;
            _ioBaseline = -1;
            _lastPct = 0;
            if (_dev || string.IsNullOrEmpty(_setupExe) || !File.Exists(_setupExe)) {
                SimulateInstall();
                return;
            }
            try {
                // NSIS /D= 必须是最后一个参数且不加引号(路径含空格也原样);
                // /_IPC 带引号, 兼容带空格的 TEMP 路径
                string ipcFlag = " /_IPC=\"" + _workDir + "\"";
                string args = "/S" + (mode == "all" ? " /allusers" : " /currentuser")
                    + " /_WV2_CONSENT=1" + ipcFlag + " /D=" + _instDir;
                var psi = new System.Diagnostics.ProcessStartInfo {
                    FileName = _setupExe,
                    Arguments = args,
                    UseShellExecute = true,
                    WorkingDirectory = Path.GetDirectoryName(_setupExe) ?? ""
                };
                if (mode == "all") psi.Verb = "runas";   // 点击时弹 UAC
                _child = System.Diagnostics.Process.Start(psi);
                if (_child == null) throw new Exception("安装器进程启动失败");
            } catch (System.ComponentModel.Win32Exception wex) {
                _installing = false;
                if (wex.NativeErrorCode == 1223) Post(MiniJson.Obj("type", "error", "msg", "已取消管理员授权"));
                else Post(MiniJson.Obj("type", "error", "msg", "无法启动安装器: " + wex.Message));
                return;
            } catch (Exception ex) {
                _installing = false;
                Post(MiniJson.Obj("type", "error", "msg", "无法启动安装器: " + ex.Message));
                return;
            }
            StartPolling();
        }

        void StartPolling() {
            long total = TotalSize();
            _poll = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(400) };
            int tick = 0;
            _poll.Tick += (s, e) => {
                tick++;
                if (_child == null) return;
                bool exited;
                try { _child.Refresh(); exited = _child.HasExited; } catch { exited = false; }

                bool extracting = File.Exists(Path.Combine(_workDir, "extract.flag"));

                // 进度优先用子进程累计写入字节(覆盖安装也准确);
                // 取不到(权限/已退出)时回退到"目录体积 - 安装前基线"。
                long written = ProcIo.WrittenBytes(_child.Id);
                if (written >= 0) {
                    if (extracting && _ioBaseline < 0) _ioBaseline = written;   // 开始写文件时定基线
                    long delta = _ioBaseline >= 0 ? written - _ioBaseline : 0;
                    if (delta > _writtenBytes) _writtenBytes = delta;
                } else {
                    long grown = DirSizeSafe(_instDir) - _baselineSize;
                    if (grown > _writtenBytes) _writtenBytes = grown;
                }
                // 留 6% 给收尾(注册表/快捷方式/自身 exe), 等 done.flag 再给 100
                int pct = total > 0
                    ? (int)Math.Min(94, _writtenBytes * 94 / total)
                    : Math.Min(94, tick);
                if (pct < _lastPct) pct = _lastPct; else _lastPct = pct;

                if (!_finished) {
                    if (File.Exists(Path.Combine(_workDir, "done.flag"))) {
                        OnInstallDone();
                    } else if (exited) {
                        if (_child.ExitCode == 0) OnInstallDone();
                        else { _installing = false; Post(MiniJson.Obj("type", "error", "msg", "安装器异常退出 (代码 " + _child.ExitCode + ")")); }
                    } else {
                        string phase = extracting ? "extract" : "prepare";
                        Post(MiniJson.Obj("type", "progress", "pct", pct.ToString(),
                            "phase", phase, "file", ""));
                    }
                }
            };
            _poll.Start();
        }

        void OnInstallDone() {
            if (_finished) return;
            _finished = true;
            _installing = false;
            Post(MiniJson.Obj("type", "progress", "pct", "100", "phase", "done", "file", "安装完成"));
            Post(MiniJson.Obj("type", "installed"));
            if (_poll != null) _poll.Stop();
            CleanupWork(false);
            if (Args.Has(_launchArgs, "auto-exit"))
                BeginInvokeShutdown();
        }

        // ── 卸载: 以 /S 重新拉起卸载器本体, 用 flag + 目录体积递减算真实进度 ──
        void StartUninstall() {
            _uninstalling = true;
            _installSize = DirSizeSafe(_instDir);
            _dataSize = DataSize();
            Log("uninstall start: purge=" + (_purgeData ? "1" : "0") + " size=" + _installSize + " data=" + _dataSize);

            if (_dev || string.IsNullOrEmpty(_uninstallExe) || !File.Exists(_uninstallExe)) {
                SimulateUninstall();
                return;
            }
            try {
                // /_IPC 带引号兼容带空格 TEMP。
                // 不能用 _?= : 卸载器原地运行时无法删除自己的 exe 与安装目录;
                // 默认的「复制到 TEMP 再运行」才能完整清理 INSTDIR。
                string ipc = " /_IPC=\"" + _workDir + "\"";
                string args = "/S /_UNINST=1 /_PURGE_DATA=" + (_purgeData ? "1" : "0") + ipc;
                var psi = new System.Diagnostics.ProcessStartInfo {
                    FileName = _uninstallExe,
                    Arguments = args,
                    UseShellExecute = true,
                    // 不能把工作目录设为 INSTDIR: 进程 CWD 会锁住目录导致 RMDir 失败
                    WorkingDirectory = _workDir
                };
                _child = System.Diagnostics.Process.Start(psi);
                if (_child == null) throw new Exception("卸载器进程启动失败");
            } catch (System.ComponentModel.Win32Exception wex) {
                _uninstalling = false;
                if (wex.NativeErrorCode == 1223) OnUninstallFailed("已取消管理员授权");
                else OnUninstallFailed("无法启动卸载器: " + wex.Message);
                return;
            } catch (Exception ex) {
                _uninstalling = false;
                OnUninstallFailed("无法启动卸载器: " + ex.Message);
                return;
            }
            StartUninstallPolling();
        }

        void StartUninstallPolling() {
            long totalInstall = Math.Max(1, _installSize);
            long totalData = _purgeData ? _dataSize : 0;
            long total = totalInstall + totalData;
            int tick = 0;
            _poll = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(400) };
            _poll.Tick += (s, e) => {
                tick++;
                if (_child == null) return;
                _child.Refresh();
                bool exited = _child.HasExited;

                long curInstall = DirSizeSafe(_instDir);
                long curData = _purgeData ? DataSize() : 0;
                long doneBytes = Math.Max(0, totalInstall - curInstall) + Math.Max(0, totalData - curData);
                // 留 4% 给收尾(注册表/快捷方式/自身 exe), 等 done.flag 再给 100
                int pct = total > 0 ? (int)Math.Min(96, doneBytes * 96 / total) : Math.Min(90, tick);

                if (_uninstalled) return;
                if (File.Exists(Path.Combine(_workDir, "done.flag"))) {
                    OnUninstallDone();
                } else if (exited) {
                    if (_child.ExitCode == 0) OnUninstallDone();
                    else OnUninstallFailed("卸载器异常退出 (代码 " + _child.ExitCode + ")");
                } else {
                    string phase = File.Exists(Path.Combine(_workDir, "purge.flag")) ? "purge"
                        : File.Exists(Path.Combine(_workDir, "extract.flag")) ? "delete"
                        : "prepare";
                    Post(MiniJson.Obj("type", "progress", "pct", pct.ToString(),
                        "phase", phase, "file", ""));
                }
            };
            _poll.Start();
        }

        void OnUninstallDone() {
            if (_uninstalled) return;
            _uninstalled = true;
            _uninstalling = false;
            Post(MiniJson.Obj("type", "progress", "pct", "100", "phase", "done", "file", "卸载完成"));
            Post(MiniJson.Obj("type", "uninstalled",
                "purge", _purgeData ? "1" : "0",
                "dataKept", _purgeData ? "0" : "1"));
            if (_poll != null) _poll.Stop();
            CleanupWork(false);
            if (Args.Has(_launchArgs, "auto-exit"))
                BeginInvokeShutdown();
        }

        void OnUninstallFailed(string msg) {
            _uninstalling = false;
            Log("uninstall failed: " + msg);
            Post(MiniJson.Obj("type", "error", "msg", msg));
        }

        // --dev 或找不到卸载器本体时的演示推进
        void SimulateUninstall() {
            int step = 0;
            _poll = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(80) };
            _poll.Tick += (s, e) => {
                step++;
                int pct = Math.Min(100, step * 3);
                string phase = pct < 10 ? "prepare" : pct < 75 ? "delete" : (_purgeData ? "purge" : "settle");
                Post(MiniJson.Obj("type", "progress", "pct", pct.ToString(), "phase", phase, "file", ""));
                if (pct >= 100) {
                    _poll.Stop();
                    _uninstalled = true;
                    _uninstalling = false;
                    Post(MiniJson.Obj("type", "uninstalled",
                        "purge", _purgeData ? "1" : "0",
                        "dataKept", _purgeData ? "0" : "1"));
                }
            };
            _poll.Start();
        }

        void BeginInvokeShutdown() {
            var t = new DispatcherTimer { Interval = TimeSpan.FromSeconds(2) };
            t.Tick += (s, e) => { t.Stop(); Shutdown(0); };
            t.Start();
        }

        // --dev 或找不到安装器本体时的演示推进
        void SimulateInstall() {
            int step = 0;
            _poll = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(80) };
            _poll.Tick += (s, e) => {
                step++;
                int pct = Math.Min(100, step * 2);
                string phase = pct < 10 ? "prepare" : pct < 90 ? "extract" : "settle";
                Post(MiniJson.Obj("type", "progress", "pct", pct.ToString(), "phase", phase, "file", ""));
                if (pct >= 100) { _poll.Stop(); _finished = true; Post(MiniJson.Obj("type", "installed")); }
            };
            _poll.Start();
        }

        // 用户数据目录(与 tauri identifier 一致)
        static string[] DataDirs() {
            return new[] {
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "com.anibox.app"),
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "com.anibox.app")
            };
        }

        static long DataSize() {
            long n = 0;
            foreach (var d in DataDirs()) n += DirSizeSafe(d);
            return n;
        }

        static long DirSizeSafe(string dir) {
            try { return DirSize(new DirectoryInfo(dir)); } catch { return 0; }
        }
        static long DirSize(DirectoryInfo d) {
            long size = 0;
            var infos = d.EnumerateFileSystemInfos();
            foreach (var fi in infos) {
                if ((fi.Attributes & System.IO.FileAttributes.ReparsePoint) != 0) continue;
                var f = fi as FileInfo;
                if (f != null) size += f.Length;
                else {
                    try { size += DirSize(fi as DirectoryInfo); } catch { }
                }
            }
            return size;
        }

        void CleanupWork(bool deleteAll) {
            try {
                if (deleteAll && Directory.Exists(_workDir)) Directory.Delete(_workDir, true);
            } catch { }
        }

        // 延迟删除 stub 资材目录(自身 exe 还锁着该目录, 用 cmd 异步删)
        static void DelayedDelete(string dir) {
            try {
                if (string.IsNullOrEmpty(dir) || !Directory.Exists(dir)) return;
                var psi = new System.Diagnostics.ProcessStartInfo(
                    "cmd.exe", "/c ping -n 3 127.0.0.1 >nul & rd /s /q \"" + dir + "\"") {
                    CreateNoWindow = true,
                    UseShellExecute = false
                };
                System.Diagnostics.Process.Start(psi);
            } catch { }
        }

        protected override void OnExit(ExitEventArgs e) {
            try {
                // 安装中途退出 UI: 子安装器继续跑完(与抖音式一致, 半途杀安装器易留脏状态)
                CleanupWork(true);
                // 清理 SetupUi 自己的工作目录 + NSIS stub 的 %TEMP%\anibox-setup-<pid> 资材目录
                DelayedDelete(_workDir);
                if (!string.IsNullOrEmpty(_wwwBase)
                    && (Path.GetFileName(_wwwBase.TrimEnd('\\')).StartsWith("anibox-setup-", StringComparison.OrdinalIgnoreCase)
                     || Path.GetFileName(_wwwBase.TrimEnd('\\')).StartsWith("anibox-uninst-", StringComparison.OrdinalIgnoreCase))
                    && _wwwBase.StartsWith(Path.GetTempPath(), StringComparison.OrdinalIgnoreCase)) {
                    DelayedDelete(_wwwBase);
                }
            } catch { }
            base.OnExit(e);
        }
    }
}
