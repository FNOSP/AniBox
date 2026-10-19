// InstallDetect.cs — 旧版 AniBox / Fntv-Plus 检测（只读）
//
// 供两处复用：
//   1. SetupUi.cs（自绘安装器 UI）在启动时调用 Detect()；
//   2. tests 下的独立只读探针（同源码编译）验证本机识别结果。
//
// 检测来源与 NSIS installer-hooks.nsh 对齐：
//   · 注册表：HKCU / HKCU(WOW6432Node) / HKLM64 / HKLM32 的
//     Software\Microsoft\Windows\CurrentVersion\Uninstall\*，
//     DisplayName 含 "AniBox" 或 "Fntv-Plus"（大小写无关）；
//   · 文件系统兜底：Electron Fntv-Plus 目录（含 Uninstall Fntv-Plus.exe）与
//     旧 Tauri AniBox 目录（含 uninstall.exe / anibox.exe）。
//
// 不写注册表、不启动任何卸载器，纯只读。
using System;
using System.Collections.Generic;
using System.IO;
using Microsoft.Win32;

namespace AniBoxSetupUi {
    internal sealed class DetectedInstall {
        public string DisplayName = "";
        public string Version = "";
        public string Dir = "";
        public string UninstallString = "";
        public string Kind = "tauri";      // "electron" = Fntv-Plus, "tauri" = 旧 AniBox
        public string Source = "";         // registry:HKCU / fs:...
        public string Root = "";           // HKCU / HKLM64 / ...
    }

    internal sealed class DetectionResult {
        public List<DetectedInstall> All = new List<DetectedInstall>();
        public List<DetectedInstall> Legacy = new List<DetectedInstall>();      // 低于当前版本
        public List<DetectedInstall> SameVersion = new List<DetectedInstall>(); // 与当前版本相同
        public DetectedInstall Primary;    // Legacy 中版本最高者
    }

    internal static class InstallDetect {
        // 与 NSIS hooks 的匹配规则一致：DisplayName 含 fntv-plus / anibox。
        public static bool IsProductName(string name) {
            if (string.IsNullOrEmpty(name)) return false;
            return name.IndexOf("anibox", StringComparison.OrdinalIgnoreCase) >= 0
                || name.IndexOf("fntv-plus", StringComparison.OrdinalIgnoreCase) >= 0
                || name.IndexOf("fntv plus", StringComparison.OrdinalIgnoreCase) >= 0;
        }

        // DisplayName 里经常带版本（"Fntv-Plus 3.9.0"）；DisplayVersion 缺失时兜底。
        public static string VersionFromName(string name) {
            if (string.IsNullOrEmpty(name)) return "";
            var m = System.Text.RegularExpressions.Regex.Match(name, @"(\d+(?:\.\d+){1,3})");
            return m.Success ? m.Groups[1].Value : "";
        }

        public static string CleanVersion(string v) {
            if (string.IsNullOrEmpty(v)) return "";
            v = v.Trim().TrimStart('v', 'V');
            int cut = v.IndexOfAny(new[] { ' ', '-', '+' });
            if (cut > 0) v = v.Substring(0, cut);
            return v.Trim();
        }

        public static bool SameVersion(string a, string b) {
            a = CleanVersion(a);
            b = CleanVersion(b);
            if (a.Length == 0 || b.Length == 0) return false;
            Version va, vb;
            if (Version.TryParse(a, out va) && Version.TryParse(b, out vb))
                return Pad(va) == Pad(vb);   // 4.0.0 == 4.0.0.0
            return string.Equals(a, b, StringComparison.OrdinalIgnoreCase);
        }

        static Version Pad(Version v) {
            return new Version(v.Major, v.Minor, v.Build < 0 ? 0 : v.Build, v.Revision < 0 ? 0 : v.Revision);
        }

        static int CompareVersionDesc(DetectedInstall x, DetectedInstall y) {
            Version vx, vy;
            bool px = Version.TryParse(CleanVersion(x.Version), out vx);
            bool py = Version.TryParse(CleanVersion(y.Version), out vy);
            if (px && py) return vy.CompareTo(vx);
            if (px) return -1;
            if (py) return 1;
            return string.Compare(y.Version, x.Version, StringComparison.OrdinalIgnoreCase);
        }

        sealed class RegRoot {
            public RegistryHive Hive;
            public RegistryView View;
            public string Name;
            public RegRoot(RegistryHive h, RegistryView v, string n) { Hive = h; View = v; Name = n; }
        }

        public static DetectionResult Detect(string currentVersion, string exeName,
                                            string defaultUserPath, string defaultAllPath) {
            var all = new List<DetectedInstall>();
            var seenDirs = new HashSet<string>(StringComparer.OrdinalIgnoreCase);

            ScanRegistry(all, seenDirs);
            ScanFileSystem(all, seenDirs, exeName, defaultUserPath, defaultAllPath);

            var result = new DetectionResult();
            result.All = all;
            foreach (var d in all) {
                if (SameVersion(d.Version, currentVersion)) result.SameVersion.Add(d);
                else result.Legacy.Add(d);
            }
            result.Legacy.Sort(CompareVersionDesc);
            // 版本相同时优先旧 Tauri AniBox（可原地覆盖），其次 Electron。
            if (result.Legacy.Count > 1) {
                result.Legacy.Sort((a, b) => {
                    int c = CompareVersionDesc(a, b);
                    if (c != 0) return c;
                    if (a.Kind == b.Kind) return 0;
                    return a.Kind == "tauri" ? -1 : 1;
                });
            }
            if (result.Legacy.Count > 0) result.Primary = result.Legacy[0];
            else if (result.SameVersion.Count > 0) result.Primary = result.SameVersion[0];
            return result;
        }

        static void ScanRegistry(List<DetectedInstall> all, HashSet<string> seenDirs) {
            var roots = new[] {
                new RegRoot(RegistryHive.CurrentUser,  RegistryView.Default,     "HKCU"),
                new RegRoot(RegistryHive.CurrentUser,  RegistryView.Registry32,  "HKCU32"),
                new RegRoot(RegistryHive.LocalMachine, RegistryView.Registry64,  "HKLM64"),
                new RegRoot(RegistryHive.LocalMachine, RegistryView.Registry32,  "HKLM32")
            };
            foreach (var root in roots) {
                try {
                    using (var baseKey = RegistryKey.OpenBaseKey(root.Hive, root.View)) {
                        using (var k = baseKey.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Uninstall")) {
                            if (k == null) continue;
                            foreach (var sub in k.GetSubKeyNames()) {
                                try {
                                    using (var sk = k.OpenSubKey(sub)) {
                                        if (sk == null) continue;
                                        string name = (sk.GetValue("DisplayName") as string) ?? "";
                                        if (!IsProductName(name)) continue;

                                        string version = CleanVersion((sk.GetValue("DisplayVersion") as string) ?? "");
                                        if (version.Length == 0) version = VersionFromName(name);

                                        string dir = ((sk.GetValue("InstallLocation") as string) ?? "").Trim().Trim('"').TrimEnd('\\');
                                        string icon = (sk.GetValue("DisplayIcon") as string) ?? "";
                                        string unstr = (sk.GetValue("UninstallString") as string) ?? "";
                                        if (dir.Length == 0) dir = DirFromDisplayIcon(icon);
                                        if (dir.Length == 0) dir = DirFromUninstallString(unstr);

                                        string kind = GuessKind(name, dir, unstr);
                                        var d = new DetectedInstall {
                                            DisplayName = name,
                                            Version = version,
                                            Dir = dir,
                                            UninstallString = unstr,
                                            Kind = kind,
                                            Source = "registry:" + root.Name,
                                            Root = root.Name
                                        };
                                        AddUnique(all, seenDirs, d);
                                    }
                                } catch { /* 单个键损坏不影响整体 */ }
                            }
                        }
                    }
                } catch { /* 视图不可用（如 32 位系统上的 Registry64） */ }
            }
        }

        static void ScanFileSystem(List<DetectedInstall> all, HashSet<string> seenDirs,
                                   string exeName, string defaultUserPath, string defaultAllPath) {
            var localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            var programFiles = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles);
            var programFilesX86 = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86);

            var candidates = new List<string>();
            if (!string.IsNullOrEmpty(localAppData)) {
                candidates.Add(Path.Combine(localAppData, "Programs", "Fntv-Plus"));
                candidates.Add(Path.Combine(localAppData, "Fntv-Plus"));
                candidates.Add(Path.Combine(localAppData, "AniBox"));
                candidates.Add(Path.Combine(localAppData, "Programs", "AniBox"));
            }
            if (!string.IsNullOrEmpty(programFiles)) {
                candidates.Add(Path.Combine(programFiles, "Fntv-Plus"));
                candidates.Add(Path.Combine(programFiles, "AniBox"));
            }
            if (!string.IsNullOrEmpty(programFilesX86)) {
                candidates.Add(Path.Combine(programFilesX86, "Fntv-Plus"));
                candidates.Add(Path.Combine(programFilesX86, "AniBox"));
            }
            // 与 SetupUi 的默认路径保持一致（即使 SpecialFolder 解析异常也覆盖）
            if (!string.IsNullOrEmpty(defaultUserPath)) candidates.Add(defaultUserPath);
            if (!string.IsNullOrEmpty(defaultAllPath)) candidates.Add(defaultAllPath);

            var seenCand = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var cand in candidates) {
                if (string.IsNullOrEmpty(cand)) continue;
                var norm = cand.TrimEnd('\\');
                if (!seenCand.Add(norm)) continue;
                if (!Directory.Exists(norm)) continue;

                bool electron = norm.IndexOf("Fntv-Plus", StringComparison.OrdinalIgnoreCase) >= 0;
                string version = "";
                string display = "";
                string unstr = "";

                if (electron) {
                    // Electron 主 exe 的 FileVersion 是 Electron 内核版本（38.x），
                    // 不能用来判断应用版本；卸载器 exe 才是应用版本（3.9.0）。
                    var uninst = Path.Combine(norm, "Uninstall Fntv-Plus.exe");
                    if (!File.Exists(uninst)) continue;   // hooks 也只认卸载器
                    version = FileVersion(uninst);
                    display = "Fntv-Plus " + version;
                    unstr = "\"" + uninst + "\" /currentuser";
                } else {
                    var uninst = Path.Combine(norm, "uninstall.exe");
                    var appExe = Path.Combine(norm, "anibox.exe");
                    if (!File.Exists(uninst) && !File.Exists(appExe)) continue; // 纯数据目录
                    version = FileVersion(appExe);
                    if (version.Length == 0) version = FileVersion(uninst);
                    display = "AniBox " + version;
                    if (File.Exists(uninst)) unstr = "\"" + uninst + "\" /S";
                }
                if (version.Length == 0) version = VersionFromName(display);

                var d = new DetectedInstall {
                    DisplayName = display.Trim(),
                    Version = CleanVersion(version),
                    Dir = norm,
                    UninstallString = unstr,
                    Kind = electron ? "electron" : "tauri",
                    Source = "fs",
                    Root = ""
                };
                AddUnique(all, seenDirs, d);
            }
        }

        static string FileVersion(string exe) {
            try {
                if (!File.Exists(exe)) return "";
                return CleanVersion(System.Diagnostics.FileVersionInfo.GetVersionInfo(exe).FileVersion ?? "");
            } catch { return ""; }
        }

        static string GuessKind(string name, string dir, string unstr) {
            if (!string.IsNullOrEmpty(name) &&
                (name.IndexOf("fntv-plus", StringComparison.OrdinalIgnoreCase) >= 0 ||
                 name.IndexOf("fntv plus", StringComparison.OrdinalIgnoreCase) >= 0))
                return "electron";
            var s = (dir ?? "") + "|" + (unstr ?? "");
            return s.IndexOf("fntv-plus", StringComparison.OrdinalIgnoreCase) >= 0 ? "electron" : "tauri";
        }

        static void AddUnique(List<DetectedInstall> all, HashSet<string> seenDirs, DetectedInstall d) {
            var key = NormalizeDir(d.Dir);
            if (key.Length == 0) key = (d.DisplayName ?? "") + "|" + (d.Version ?? "");
            if (!seenDirs.Add(key)) {
                // 同一安装目录在多个注册表视图出现时，保留信息更完整的一条。
                foreach (var e in all) {
                    if (NormalizeDir(e.Dir) == NormalizeDir(d.Dir) && NormalizeDir(d.Dir).Length > 0) {
                        if (string.IsNullOrEmpty(e.UninstallString)) e.UninstallString = d.UninstallString;
                        if (string.IsNullOrEmpty(e.Version)) e.Version = d.Version;
                        return;
                    }
                }
                return;
            }
            all.Add(d);
        }

        static string NormalizeDir(string dir) {
            if (string.IsNullOrEmpty(dir)) return "";
            return dir.Trim().Trim('"').TrimEnd('\\').ToLowerInvariant();
        }

        static string DirFromDisplayIcon(string s) {
            if (string.IsNullOrEmpty(s)) return "";
            var p = s.Split(',')[0].Trim().Trim('"');
            if (p.Length < 4) return "";
            try { return (Path.GetDirectoryName(p) ?? "").TrimEnd('\\'); } catch { return ""; }
        }

        static string DirFromUninstallString(string s) {
            if (string.IsNullOrEmpty(s)) return "";
            string exe = "";
            var m = System.Text.RegularExpressions.Regex.Match(s, "\"([^\"]+)\"");
            if (m.Success) {
                exe = m.Groups[1].Value;
            } else {
                int idx = s.IndexOf(".exe", StringComparison.OrdinalIgnoreCase);
                if (idx >= 0) exe = s.Substring(0, idx + 4).Trim();
            }
            if (exe.Length == 0) return "";
            try { return (Path.GetDirectoryName(exe) ?? "").TrimEnd('\\'); } catch { return ""; }
        }
    }
}
