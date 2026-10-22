// WebView2Detect.cs — Microsoft Edge WebView2 Runtime 检测 / 在线安装引导（只读检测 + 用户同意后安装）
//
// 为什么必须放在自绘 UI 之前：
//   AniBox 的安装器界面（AniBoxSetupUi.exe）本身就是 WPF + WebView2 绘制，
//   运行时缺失时连窗口都渲染不出来。因此检测必须在创建 WebView2 之前完成，
//   用原生 MessageBox 提示用户，得到同意后再下载安装，安装成功后重新检测，
//   仍然缺失则中止安装并给出手动安装指引。
//
// 检测口径（任一命中即视为已安装，版本取最大者）：
//   · 注册表：HKLM / HKCU 的
//       SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}\pv
//     以及 WOW6432Node 下的同名路径（32/64 位双视图）；
//     兼容 WebView2 预览通道（Client ID 前缀 {F1E7B9A0）与 name 含 WebView2 的项。
//   · 目录：%ProgramFiles% / %ProgramFiles(x86)% / %LOCALAPPDATA%
//       \Microsoft\EdgeWebView\Application\<版本>\msedgewebview2.exe
//
// 在线安装（用户明确同意后才执行）：
//   cmd /c curl.exe -fL -o "%TEMP%\MicrosoftEdgeWebview2Setup.exe" "https://go.microsoft.com/fwlink/p/?LinkId=2124703"
//   失败回退 powershell Invoke-WebRequest（TLS1.2）
//   cmd /c ""%TEMP%\MicrosoftEdgeWebview2Setup.exe" /silent /install"
//   安装后轮询重新检测；仍缺失 → 中止并提示手动下载链接。
//
// 不写注册表、不改系统设置；只下载并运行微软官方 Evergreen Bootstrapper。
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Threading;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Threading;
using Microsoft.Win32;

namespace AniBoxSetupUi {
    internal sealed class WebView2Info {
        public bool Installed;
        public string Version = "";
        public string Source = "none";
        public string Path = "";

        public bool SatisfiesMinimum {
            get { return Installed && WebView2Runtime.IsAtLeastMinimum(Version); }
        }
    }

    internal static class WebView2Runtime {
        // 与 tauri.conf.json 的 minimumWebview2Version 保持一致
        public const string MinimumVersion = "109.0.0.0";
        public const string RuntimeClientGuid = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
        public const string BootstrapperUrl = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";
        public const string ManualDownloadUrl = "https://developer.microsoft.com/zh-cn/microsoft-edge/webview2/";

        public static bool IsAtLeastMinimum(string version) {
            Version v, min;
            if (!Version.TryParse((version ?? "").Trim(), out v)) return false;
            if (!Version.TryParse(MinimumVersion, out min)) return false;
            return v >= min;
        }

        // ── 检测 ────────────────────────────────────────────────────────────
        public static WebView2Info Check() {
            var info = new WebView2Info();
            Version best = null;
            string bestSource = "none";
            string bestPath = "";

            Version reg = ScanRegistry();
            if (reg != null) { best = reg; bestSource = "registry"; }

            string dirPath;
            Version dir = ScanDirectories(out dirPath);
            if (dir != null && (best == null || dir > best)) {
                best = dir;
                bestSource = bestSource == "registry" ? "registry+directory" : "directory";
                bestPath = dirPath;
            } else if (best != null && dir != null) {
                bestPath = dirPath;
            }

            if (best != null) {
                info.Installed = true;
                info.Version = best.ToString();
                info.Source = bestSource;
                info.Path = bestPath;
            }
            return info;
        }

        static bool IsWebView2ClientKey(string id, string displayName) {
            if (string.IsNullOrEmpty(id)) return false;
            if (string.Equals(id, RuntimeClientGuid, StringComparison.OrdinalIgnoreCase)) return true;
            // WebView2 预览/Canary 通道的 Client ID 前缀（与 tools/ensure-webview2.ps1 一致）
            if (id.StartsWith("{F1E7B9A0", StringComparison.OrdinalIgnoreCase)) return true;
            if (!string.IsNullOrEmpty(displayName)
                && displayName.IndexOf("webview2", StringComparison.OrdinalIgnoreCase) >= 0) return true;
            return false;
        }

        static Version ScanRegistry() {
            // 显式使用 32/64 位视图，避免 32 位进程被 WOW6432Node 重定向后漏检
            var targets = new[] {
                new object[] { RegistryHive.LocalMachine, RegistryView.Registry64 },
                new object[] { RegistryHive.LocalMachine, RegistryView.Registry32 },
                new object[] { RegistryHive.CurrentUser,  RegistryView.Registry64 },
                new object[] { RegistryHive.CurrentUser,  RegistryView.Registry32 },
            };
            Version best = null;
            foreach (var t in targets) {
                try {
                    using (var baseKey = RegistryKey.OpenBaseKey((RegistryHive)t[0], (RegistryView)t[1]))
                    using (var clients = baseKey.OpenSubKey(@"SOFTWARE\Microsoft\EdgeUpdate\Clients")) {
                        if (clients == null) continue;
                        foreach (var id in clients.GetSubKeyNames()) {
                            try {
                                using (var key = clients.OpenSubKey(id)) {
                                    if (key == null) continue;
                                    string pv = key.GetValue("pv") as string;
                                    string name = key.GetValue("name") as string;
                                    if (!IsWebView2ClientKey(id, name)) continue;
                                    Version v;
                                    if (!Version.TryParse((pv ?? "").Trim(), out v)) continue;
                                    if (best == null || v > best) best = v;
                                }
                            } catch { }
                        }
                    }
                } catch { }
            }
            return best;
        }

        static Version ScanDirectories(out string bestExe) {
            bestExe = "";
            var roots = new List<string>();
            try {
                string pf = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles);
                if (!string.IsNullOrEmpty(pf)) roots.Add(Path.Combine(pf, @"Microsoft\EdgeWebView\Application"));
            } catch { }
            try {
                string pf86 = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86);
                if (!string.IsNullOrEmpty(pf86)) roots.Add(Path.Combine(pf86, @"Microsoft\EdgeWebView\Application"));
            } catch { }
            try {
                string lad = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
                if (!string.IsNullOrEmpty(lad)) roots.Add(Path.Combine(lad, @"Microsoft\EdgeWebView\Application"));
            } catch { }

            Version best = null;
            foreach (var root in roots) {
                try {
                    if (!Directory.Exists(root)) continue;
                    foreach (var dir in Directory.GetDirectories(root)) {
                        string exe = Path.Combine(dir, "msedgewebview2.exe");
                        if (!File.Exists(exe)) continue;
                        Version v;
                        if (!Version.TryParse(Path.GetFileName(dir).Trim(), out v)) continue;
                        if (best == null || v > best) { best = v; bestExe = exe; }
                    }
                } catch { }
            }
            return best;
        }

        // ── 交互：提示 → 同意 → 在线安装 → 重检 ─────────────────────────────
        // 返回 true 表示运行时可用，可以继续安装/卸载流程。
        // uninstallContext: 卸载界面同样依赖 WebView2，用不同文案提示。
        public static bool EnsureInteractive(bool uninstallContext) {
            var info = Check();
            if (info.SatisfiesMinimum) return true;

            bool outdated = info.Installed;
            string title = uninstallContext ? "AniBox 卸载 - 缺少 WebView2 运行时" : "AniBox 安装 - 缺少 WebView2 运行时";
            string msg = outdated
                ? "检测到 Microsoft Edge WebView2 Runtime 版本过低（v" + info.Version + "，最低需要 v" + MinimumVersion + "）。\n\n"
                  + "缺少可用的 Microsoft Edge WebView2 Runtime，" + (uninstallContext ? "卸载界面" : "AniBox") + "将无法运行。\n\n"
                  + "是否立即在线下载并安装 WebView2 Runtime？"
                : "未检测到 Microsoft Edge WebView2 Runtime。\n\n"
                  + "缺少 Microsoft Edge WebView2 Runtime，" + (uninstallContext ? "卸载界面" : "AniBox") + "将无法运行。\n\n"
                  + "是否立即在线下载并安装 WebView2 Runtime？（需要联网，约 1~2 分钟）";

            var answer = MessageBox.Show(msg, title, MessageBoxButton.YesNo, MessageBoxImage.Warning, MessageBoxResult.Yes);
            if (answer != MessageBoxResult.Yes) {
                ShowManualHelp(uninstallContext
                    ? "您选择了不安装 WebView2 Runtime，卸载界面无法启动。"
                    : "您选择了不安装 WebView2 Runtime，安装已中止。");
                return false;
            }

            string error;
            if (InstallWithProgress(out error)) {
                MessageBox.Show("Microsoft Edge WebView2 Runtime 安装成功，继续" + (uninstallContext ? "卸载。" : "安装。"),
                    title, MessageBoxButton.OK, MessageBoxImage.Information);
                return true;
            }

            MessageBox.Show("WebView2 Runtime 安装失败：" + error + "\n\n"
                + "安装已中止。您可以手动下载安装后重新运行本程序。",
                title, MessageBoxButton.OK, MessageBoxImage.Error);
            ShowManualHelp("手动安装指引");
            return false;
        }

        static void ShowManualHelp(string headline) {
            MessageBox.Show(headline + "\n\n"
                + "手动安装方法：\n"
                + "1. 打开官方下载页：" + ManualDownloadUrl + "\n"
                + "2. 下载「Evergreen Bootstrapper」并运行；\n"
                + "3. 安装完成后重新运行 AniBox 安装程序。\n\n"
                + "直接下载链接：\n" + BootstrapperUrl,
                "AniBox - WebView2 手动安装指引", MessageBoxButton.OK, MessageBoxImage.Information);
        }

        // 进度窗口 + 后台安装线程；用 DispatcherFrame 保持 UI 响应
        static bool InstallWithProgress(out string error) {
            error = "";
            bool ok = false;
            string localError = "";

            Window win = null;
            try {
                win = BuildProgressWindow();
                win.Show();
            } catch { win = null; }

            var dispatcher = Dispatcher.CurrentDispatcher;
            var frame = new DispatcherFrame();
            var worker = new Thread(() => {
                try {
                    ok = DoInstall(out localError);
                } catch (Exception ex) {
                    localError = ex.Message;
                    ok = false;
                } finally {
                    dispatcher.BeginInvoke(new Action(() => { frame.Continue = false; }), DispatcherPriority.Background);
                }
            });
            worker.IsBackground = true;
            worker.Start();
            Dispatcher.PushFrame(frame);

            try { if (win != null) win.Close(); } catch { }
            error = localError;
            return ok;
        }

        static Window BuildProgressWindow() {
            var panel = new StackPanel { Margin = new Thickness(20) };
            panel.Children.Add(new TextBlock {
                Text = "正在下载并安装 Microsoft Edge WebView2 Runtime…",
                FontSize = 14,
                Margin = new Thickness(0, 0, 0, 10)
            });
            var bar = new ProgressBar { IsIndeterminate = true, Height = 8, Width = 360 };
            panel.Children.Add(bar);
            panel.Children.Add(new TextBlock {
                Text = "需要联网，安装过程可能需要 1~2 分钟，请勿关闭窗口。",
                FontSize = 12,
                Foreground = System.Windows.Media.Brushes.Gray,
                Margin = new Thickness(0, 10, 0, 0)
            });
            return new Window {
                Title = "AniBox - 安装 WebView2 运行时",
                Content = panel,
                SizeToContent = SizeToContent.WidthAndHeight,
                WindowStartupLocation = WindowStartupLocation.CenterScreen,
                ResizeMode = ResizeMode.NoResize,
                WindowStyle = WindowStyle.ToolWindow,
                ShowInTaskbar = true,
                Topmost = true
            };
        }

        // 下载 + 静默安装 + 重检（后台线程，不碰 UI）
        static bool DoInstall(out string error) {
            error = "";
            string exe = Path.Combine(Path.GetTempPath(), "MicrosoftEdgeWebview2Setup.exe");
            try { if (File.Exists(exe)) File.Delete(exe); } catch { }

            // 1) 下载：cmd + curl 优先（系统自带 curl.exe），失败回退 PowerShell
            if (!TryDownload(exe, out error)) {
                try { if (File.Exists(exe)) File.Delete(exe); } catch { }
                return false;
            }

            // 2) 静默安装
            try {
                var psi = new ProcessStartInfo {
                    FileName = "cmd.exe",
                    Arguments = "/c \"\"" + exe + "\" /silent /install\"",
                    UseShellExecute = false,
                    CreateNoWindow = true,
                    WindowStyle = ProcessWindowStyle.Hidden
                };
                using (var p = Process.Start(psi)) {
                    if (p == null) { error = "无法启动 WebView2 安装程序"; return false; }
                    if (!p.WaitForExit(15 * 60 * 1000)) {
                        error = "安装超时（超过 15 分钟）";
                        try { p.Kill(); } catch { }
                        return false;
                    }
                    // 0=成功；3010=成功但需要重启；其它视为失败
                    if (p.ExitCode != 0 && p.ExitCode != 3010) {
                        error = "安装程序返回错误码 " + p.ExitCode;
                        return false;
                    }
                }
            } catch (Exception ex) {
                error = "启动安装程序失败：" + ex.Message;
                return false;
            } finally {
                try { if (File.Exists(exe)) File.Delete(exe); } catch { }
            }

            // 3) 重新检测（注册表写入可能有延迟，最多等 20 秒）
            for (int i = 0; i < 10; i++) {
                Thread.Sleep(2000);
                var info = Check();
                if (info.SatisfiesMinimum) return true;
            }
            error = "安装完成后仍未检测到可用的 WebView2 Runtime（版本需 ≥ " + MinimumVersion + "）";
            return false;
        }

        static bool TryDownload(string target, out string error) {
            error = "";

            // curl.exe（Win10 1803+ 自带）
            try {
                var psi = new ProcessStartInfo {
                    FileName = "cmd.exe",
                    Arguments = "/c curl.exe -fL --retry 2 --connect-timeout 20 -o \"" + target + "\" \"" + BootstrapperUrl + "\"",
                    UseShellExecute = false,
                    CreateNoWindow = true,
                    WindowStyle = ProcessWindowStyle.Hidden
                };
                using (var p = Process.Start(psi)) {
                    if (p != null) {
                        p.WaitForExit(5 * 60 * 1000);
                        if (p.ExitCode == 0 && IsUsableInstaller(target)) return true;
                    }
                }
            } catch { }

            // 回退：PowerShell Invoke-WebRequest（TLS 1.2）
            try {
                try { if (File.Exists(target)) File.Delete(target); } catch { }
                string ps = "$ProgressPreference='SilentlyContinue';"
                    + "[Net.ServicePointManager]::SecurityProtocol=[Net.SecurityProtocolType]::Tls12;"
                    + "Invoke-WebRequest -Uri '" + BootstrapperUrl + "' -OutFile '" + target + "' -UseBasicParsing";
                var psi = new ProcessStartInfo {
                    FileName = "powershell.exe",
                    Arguments = "-NoProfile -ExecutionPolicy Bypass -Command \"" + ps.Replace("\"", "\\\"") + "\"",
                    UseShellExecute = false,
                    CreateNoWindow = true,
                    WindowStyle = ProcessWindowStyle.Hidden
                };
                using (var p = Process.Start(psi)) {
                    if (p != null) {
                        p.WaitForExit(5 * 60 * 1000);
                        if (p.ExitCode == 0 && IsUsableInstaller(target)) return true;
                    }
                }
            } catch { }

            error = "无法从微软官方地址下载 WebView2 安装器（网络、代理或防火墙限制）";
            return false;
        }

        static bool IsUsableInstaller(string path) {
            try {
                var fi = new FileInfo(path);
                return fi.Exists && fi.Length > 200 * 1024;   // 正常安装器约 1.7 MB
            } catch { return false; }
        }
    }
}
