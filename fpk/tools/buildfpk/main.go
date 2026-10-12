// buildfpk —— Fntv-Plus fpk 打包引擎（无界面 CLI，供打包 GUI / 脚本调用）。
//
// 编译（在 fpk/ 目录）：go build -o build-fpk.exe ./tools/buildfpk
// GUI：双击 fpk/打包.bat（PowerShell WinForms 界面，调本引擎并实时显示输出）。
//
// 用法：
//	build-fpk.exe build   [--arch amd64|arm64] [--out <目录>]                开发测试版（版号自动 = fpk 提交数/痕迹）
//	build-fpk.exe release --name <显示名> --version x.y.z [--arch amd64|arm64] [--out <目录>]   正式发布版（飞牛商店口径）
//
// 架构：默认 amd64（manifest platform=x86）；--arch arm64 交叉编译 linux/arm64 并把 manifest platform 写成
//       arm，产物加 -arm64 后缀（fnOS 商店 platform 三选一：x86 / arm / all，见 lc-1387）。
//       包内只有 app/server/fntvplus 一个原生二进制，所以 ARM 包 = 同路径放 arm64 二进制 + platform=arm；
//       单次构建结束时会把该路径还原为 amd64（保持工作区默认形态）。
// 流程：重建 payload(esbuild) → 交叉编译后端(linux/<arch>) → fnpack 打包
//       → 产物移动到 --out 目录（默认 <仓库根>/release）。
// 任何一步缺工具（node/go/fnpack）都会降级或给出明确提示；退出码 0=成功 1=失败。
package main

import (
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

var root string

// —— 架构支持（lc-1387）——
const (
	archAMD64 = "amd64"
	archARM64 = "arm64"
)

// normalizeArch 归一化 --arch 取值（兼容 aarch64/x86_64/arm 等写法）。
func normalizeArch(a string) (string, error) {
	switch strings.ToLower(strings.TrimSpace(a)) {
	case "", "amd64", "x86_64", "x64", "x86":
		return archAMD64, nil
	case "arm64", "aarch64", "arm":
		return archARM64, nil
	}
	return "", fmt.Errorf("未知架构 %q（仅支持 amd64 / arm64）", a)
}

// archSuffix 产物文件名后缀（amd64 不加，保持历史命名 Fntv-Plus-<版号>.fpk 不变）。
func archSuffix(arch string) string {
	if arch == archARM64 {
		return "-arm64"
	}
	return ""
}

// platformForArch fnOS manifest 的 platform 取值：amd64→x86、arm64→arm。
func platformForArch(arch string) string {
	if arch == archARM64 {
		return "arm"
	}
	return "x86"
}

func main() {
	// 参数解析（手工，保持零依赖）：
	//   build                          开发测试版
	//   release --name X --version x.y.z   正式发布版
	//   --arch amd64|arm64             目标架构（默认 amd64）
	//   --out <dir>                    产物输出目录（默认 <仓库根>/release）
	mode := "build"
	outDir := ""
	relName := ""
	relVer := ""
	archArg := ""
	args := os.Args[1:]
	for i := 0; i < len(args); i++ {
		a := args[i]
		switch {
		case a == "build":
			mode = "build"
		case a == "release":
			mode = "release"
		case a == "--out" && i+1 < len(args):
			i++
			outDir = args[i]
		case a == "--name" && i+1 < len(args):
			i++
			relName = args[i]
		case a == "--version" && i+1 < len(args):
			i++
			relVer = args[i]
		case a == "--arch" && i+1 < len(args):
			i++
			archArg = args[i]
		default:
			fmt.Printf("[X] 未知参数: %s\n", a)
			os.Exit(1)
		}
	}
	arch, err := normalizeArch(archArg)
	if err != nil {
		fmt.Printf("[X] %v\n", err)
		os.Exit(1)
	}
	if err := run(mode, relName, relVer, outDir, arch); err != nil {
		fmt.Printf("\n[X] %v\n", err)
		os.Exit(1)
	}
	os.Exit(0)
}

// defaultOutDir 产物默认输出目录 = 仓库根/release（engine 位于 <仓库根>/fpk/tools/）。
func defaultOutDir() string {
	return filepath.Join(filepath.Dir(root), "release")
}

func run(mode, relName, relVer, outDir, arch string) error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	root = filepath.Dir(exe)
	if _, err := os.Stat(filepath.Join(root, "manifest")); err != nil {
		return fmt.Errorf("在 %s 找不到 manifest，请把 build-fpk.exe 放在 fpk/ 目录（Fntv-Plus 仓库内 Web 版子目录）", root)
	}
	if outDir == "" {
		outDir = defaultOutDir()
	}
	if err := os.MkdirAll(outDir, 0o755); err != nil {
		return fmt.Errorf("创建输出目录失败: %w", err)
	}

	fmt.Println("==============================================")
	fmt.Println(" Fntv-Plus fpk 打包（" + map[bool]string{true: "正式发布版", false: "开发测试版"}[mode == "release"] + "）")
	fmt.Println("==============================================")
	fmt.Printf("项目根: %s\n", root)
	fmt.Printf("输出目录: %s\n", outDir)
	fmt.Printf("目标平台: %s（linux/%s，manifest platform=%s）\n\n", arch, arch, platformForArch(arch))

	// [lc-1387] 打包前把 manifest 的 platform 写成目标架构，打包完成后还原：
	//   fnOS 商店 platform 三选一（x86 / arm / all），包内二进制必须与之一致。
	origPlatform := manifestGetString("platform", "x86")
	if err := manifestSetString("platform", platformForArch(arch)); err != nil {
		return err
	}
	defer func() {
		if origPlatform != "" && origPlatform != platformForArch(arch) {
			_ = manifestSetString("platform", origPlatform)
			fmt.Printf("（已还原 manifest platform=%s）\n", origPlatform)
		}
	}()

	var final string
	if mode == "release" {
		final, err = runRelease(relName, relVer, arch)
	} else {
		final, err = runDev(arch)
	}
	if err != nil {
		return err
	}

	// [lc-1387] 包内只有 app/server/fntvplus 一个原生二进制：arm64 构建后把该路径还原为 amd64，
	// 避免工作区里留着一个 ARM 二进制（下次构建本来也会重编，但状态一致更不易误判）。
	if arch != archAMD64 {
		bin := filepath.Join(root, "app", "server", "fntvplus")
		if goBin, _ := exec.LookPath("go"); goBin != "" {
			if err := buildBackend(bin, archAMD64); err != nil {
				fmt.Printf("[warn] 还原 amd64 后端失败（不影响已完成的 ARM 包）: %v\n", err)
			}
		}
	}

	// 产物移动到输出目录（跨盘 rename 失败时退回复制+删除）
	base := filepath.Base(final)
	dst := filepath.Join(outDir, base)
	if dst != final {
		if err := os.Rename(final, dst); err != nil {
			if err2 := copyFile(final, dst); err2 != nil {
				return fmt.Errorf("移动产物到输出目录失败: %v / %v", err, err2)
			}
			_ = os.Remove(final)
		}
	}
	st, err := os.Stat(dst)
	sizeMB := "0"
	if err == nil {
		sizeMB = fmt.Sprintf("%.1f", float64(st.Size())/1024/1024)
	}
	fmt.Println("\n==============================================")
	fmt.Println("[OK] 打包完成！")
	fmt.Printf("  产物: %s (%s MB)\n", dst, sizeMB)
	fmt.Printf("  版本: %s\n", manifestVersion())
	fmt.Println("  安装: 飞牛 fnOS 应用中心 → 手动安装 → 上传该 .fpk。")
	fmt.Println("==============================================")
	return nil
}

// runDev 开发测试版：版号自动（max(fpk 提交数, 痕迹+1)+1，严格递增）。
func runDev(arch string) (string, error) {
	devVer := devCommitVersion()
	writeVersion(devVer)
	fmt.Printf("开发版号: %s\n", devVer)
	return buildPackage("dev", arch)
}

// runRelease 正式发布版：显示名与版号完全由调用方控制（飞牛商店真实展示）。
// 版号须 x.y.z 三段数字；产物 Fntv-Plus-<版号>[-arm64].fpk（不带 v/V 前缀，用户指定统一命名）。
func runRelease(relName, relVer, arch string) (string, error) {
	if strings.TrimSpace(relName) == "" {
		return "", fmt.Errorf("正式发布需要 --name <显示名>")
	}
	if !regexp.MustCompile(`^\d+\.\d+\.\d+$`).MatchString(relVer) {
		return "", fmt.Errorf("版号须为 x.y.z 三段数字（当前输入: %q）", relVer)
	}
	// [v1.6.3] 双轨版号: release_version = 正式版号；version = 本次安装包实际版本号
	if err := manifestSetString("display_name", relName); err != nil {
		return "", err
	}
	if err := manifestSetString("release_version", relVer); err != nil {
		return "", err
	}
	if err := manifestSetString("version", relVer); err != nil {
		return "", err
	}
	fmt.Printf("正式版: %s @ %s（%s）\n", relName, relVer, arch)
	final, err := buildPackage("rel", arch)
	if err != nil {
		return "", err
	}
	// [命名统一] 不再用大小写 v/V 区分开发/发布包：发布包 = Fntv-Plus-<版号>[-arm64].fpk
	named := filepath.Join(root, "Fntv-Plus-"+relVer+archSuffix(arch)+".fpk")
	if err := os.Rename(final, named); err != nil {
		return final, nil // 重命名失败不致命，返回原名
	}
	return named, nil
}

// buildPackage 完整打包链路：payload → 后端(<arch>) → fnpack → 本地命名（dev/rel）。
func buildPackage(kind, arch string) (string, error) {
	syncPayload()
	// fnpack 会把 app/ 整个打进去：若残留 build-fpk.sh 时代留下的 fntvplus-arm64，
	// 每个包都会白胖 ~10MB 且与 manifest platform 语义打架，这里明确提示。
	if stale := filepath.Join(root, "app", "server", "fntvplus-arm64"); func() bool {
		_, err := os.Stat(stale)
		return err == nil
	}() {
		fmt.Printf("[warn] 检测到 %s（旧 build-fpk.sh 产物）：会被一并打进 .fpk，建议删除；ARM 包请用 --arch arm64\n", stale)
	}
	bin := filepath.Join(root, "app", "server", "fntvplus")
	if err := buildBackend(bin, arch); err != nil {
		return "", err
	}
	fpk, err := pack(arch)
	if err != nil {
		return "", err
	}
	// [命名统一] 带序号包命名: Fntv-Plus-<序号>[-arm64].fpk（不再用大小写 v/V 区分开发/发布）。
	// 版号 x.y.z-<seq> → 取 "-" 后数字。
	if ver := manifestVersion(); strings.Contains(ver, "-") {
		if n, err := strconv.Atoi(strings.TrimPrefix(ver, strings.SplitN(ver, "-", 2)[0]+"-")); err == nil {
			named := filepath.Join(root, "Fntv-Plus-"+strconv.Itoa(n)+archSuffix(arch)+".fpk")
			if err := os.Rename(fpk, named); err == nil {
				cleanOldDevPackages(named, arch)
				return named, nil
			}
		}
	}
	return fpk, nil
}

// syncPayload 若 node 可用则重建 dist/fntv-plus.user.js 并同步到 Go embed 目录；
// node 不可用则沿用仓库里已提交的 embed payload。
func syncPayload() {
	node, _ := exec.LookPath("node")
	if node == "" {
		fmt.Println("[skip] 未找到 node，使用已内置的 payload（如改了网页端源码请装 node 后重跑）")
		return
	}
	script := filepath.Join(root, "scripts", "build-userjs.mjs")
	if _, err := os.Stat(script); err != nil {
		fmt.Println("[skip] scripts/build-userjs.mjs 不存在，跳过 payload 重建")
		return
	}
	fmt.Println("[1/3] 重建网页端 payload (esbuild)...")
	cmd := exec.Command(node, script)
	cmd.Dir = root
	cmd.Stdout, cmd.Stderr = os.Stdout, os.Stderr
	if err := cmd.Run(); err != nil {
		fmt.Printf("[warn] payload 重建失败（%v），继续使用现有 payload\n", err)
		return
	}
	dist := filepath.Join(root, "dist", "fntv-plus.user.js")
	embed := filepath.Join(root, "src", "go", "internal", "inject", "payload", "fntv-plus.user.js")
	if err := copyFile(dist, embed); err != nil {
		fmt.Printf("[warn] payload 同步到 embed 目录失败: %v\n", err)
		return
	}
	fmt.Println("       dist/fntv-plus.user.js -> src/go/internal/inject/payload/ 同步完成")
}

// buildBackend 交叉编译 linux/<arch> 后端到 app/server/fntvplus。
// 注意：包内只有这一个原生二进制（fnpack 只打 app/ 目录），所以 arm64 构建同样是覆盖同一路径；
// 缺 go 时只在 amd64 下复用既有二进制（绝不复用 amd64 产物冒充 arm，那会打出装不起来的包）。
func buildBackend(bin, arch string) error {
	fmt.Printf("[2/3] 交叉编译后端 (linux/%s)...\n", arch)
	goBin, _ := exec.LookPath("go")
	if goBin == "" {
		if arch == archAMD64 {
			if _, err := os.Stat(bin); err == nil {
				fmt.Println("[skip] 未找到 go，复用已有 app/server/fntvplus（amd64）")
				return nil
			}
		}
		return fmt.Errorf("未找到 go 工具链，无法交叉编译 linux/%s 后端；请安装 Go 1.23+ 后重跑", arch)
	}
	if err := os.MkdirAll(filepath.Dir(bin), 0o755); err != nil {
		return err
	}
	// 先编译到 src/go 目录内（个别环境下 -o 指向目录外会静默失败），再移动到 app/server/
	tmpName := "fntvplus_linux_" + arch
	tmp := filepath.Join(root, "src", "go", tmpName)
	// [lc-167] 编译期注入版号兜底：-X main.buildVersion=<manifest version>
	args := []string{"build"}
	if v := manifestVersion(); v != "" && v != "?" {
		args = append(args, "-ldflags", "-X main.buildVersion="+v)
	}
	args = append(args, "-o", tmpName, "./cmd/fntvplus")
	cmd := exec.Command(goBin, args...)
	cmd.Dir = filepath.Join(root, "src", "go")
	cmd.Env = append(os.Environ(), "GOOS=linux", "GOARCH="+arch, "CGO_ENABLED=0")
	cmd.Stdout, cmd.Stderr = os.Stdout, os.Stderr
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("后端编译失败 (linux/%s): %w", arch, err)
	}
	if _, err := os.Stat(tmp); err != nil {
		return fmt.Errorf("编译命令成功但产物缺失: %s", tmp)
	}
	if err := os.Rename(tmp, bin); err != nil {
		if err2 := copyFile(tmp, bin); err2 != nil {
			return fmt.Errorf("移动编译产物失败: %v / %v", err, err2)
		}
		_ = os.Remove(tmp)
	}
	if _, err := os.Stat(bin); err != nil {
		return fmt.Errorf("编译产物最终缺失: %s", bin)
	}
	fmt.Printf("       app/server/fntvplus 编译完成 (linux/%s)\n", arch)
	return nil
}

// pack 调 fnpack build 打出 .fpk 并本地命名（含架构后缀）。
func pack(arch string) (string, error) {
	fnpack, err := findFnpack()
	if err != nil {
		return "", err
	}
	fmt.Printf("[3/3] fnpack 打包 (%s)...\n", fnpack)

	old := filepath.Join(root, "fntvplus.fpk")
	_ = os.Remove(old)

	cmd := exec.Command(fnpack, "build")
	cmd.Dir = root
	cmd.Stdout, cmd.Stderr = os.Stdout, os.Stderr
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("fnpack build 失败: %w", err)
	}
	out := filepath.Join(root, "fntvplus.fpk")
	if st, err := os.Stat(out); err != nil || st.Size() < 1024 {
		return "", fmt.Errorf("fnpack 未产出有效的 fntvplus.fpk（请检查上方报错）")
	}
	if ver := manifestVersion(); strings.Contains(ver, "-") {
		if n, err := strconv.Atoi(strings.TrimPrefix(ver, strings.SplitN(ver, "-", 2)[0]+"-")); err == nil {
			// [命名统一] 测试包 = Fntv-Plus-<序号>[-arm64].fpk（仅序号，无 v 前缀）
			named := filepath.Join(root, "Fntv-Plus-"+strconv.Itoa(n)+archSuffix(arch)+".fpk")
			if err := os.Rename(out, named); err == nil {
				cleanOldDevPackages(named, arch)
				return named, nil
			}
		}
	}
	return out, nil
}

// cleanOldDevPackages 清理 root 与输出目录里的旧测试包（新命名 Fntv-Plus-<序号>[-arm64].fpk、
// 历史 Fntv-Plus-v<序号>.fpk）；发布包 Fntv-Plus-x.y.z.fpk 带点，正则天然不匹配、不会误删。
// 只清同架构的包（-arm64 与 amd64 互不干扰）。outDir 可为空（不扫）。
func cleanOldDevPackages(keep, arch string) {
	// 兼容历史命名（Fntv-Plus-v123.fpk）与新命名（Fntv-Plus-123.fpk / Fntv-Plus-123-arm64.fpk）；
	// 纯数字形态不会与发布包冲突（发布包是 x.y.z 带点）。
	pat := `^Fntv-Plus-(?:v)?\d+\.fpk$`
	if arch == archARM64 {
		pat = `^Fntv-Plus-(?:v)?\d+-arm64\.fpk$`
	}
	re := regexp.MustCompile(pat)
	removed, checked := 0, 0
	for _, dir := range []string{root, filepath.Dir(keep)} {
		entries, err := os.ReadDir(dir)
		if err != nil {
			continue
		}
		for _, e := range entries {
			name := e.Name()
			if e.IsDir() || !strings.HasPrefix(name, "Fntv-Plus-") || !strings.HasSuffix(name, ".fpk") {
				continue
			}
			if !re.MatchString(name) {
				continue
			}
			p := filepath.Join(dir, name)
			if p == keep {
				continue
			}
			checked++
			if err := os.Remove(p); err == nil {
				fmt.Printf("       已清理旧包: %s\n", name)
				removed++
			}
		}
	}
	if removed == 0 && checked == 0 {
		fmt.Println("       无旧开发包需要清理")
	}
}

// findFnpack 依次查找 tools/fnpack.exe、用户 Downloads 下的 fnpack*。
func findFnpack() (string, error) {
	local := filepath.Join(root, "tools", "fnpack.exe")
	if _, err := os.Stat(local); err == nil {
		return local, nil
	}
	home, _ := os.UserHomeDir()
	hits, _ := filepath.Glob(filepath.Join(home, "Downloads", "fnpack*"))
	sort.Strings(hits)
	for _, h := range hits {
		if st, err := os.Stat(h); err == nil && !st.IsDir() {
			return h, nil
		}
	}
	return "", fmt.Errorf("找不到 fnpack：请把 fnpack-<ver>-windows-amd64 复制为 %s（或放到 Downloads 下）",
		filepath.Join("fpk", "tools", "fnpack.exe"))
}

// devCommitVersion 开发测试版号 = <发布版基号>-<序号>，严格递增（数字预发布后缀形态）。
// 序号取三者最大：git 提交数（只数 fpk/ 目录）、历史痕迹最大包号+1、manifest 当前序号。
func devCommitVersion() string {
	base := manifestGetString("release_version", "1.0.0")
	cur := 0
	if ver := manifestVersion(); strings.HasPrefix(ver, base+"-") {
		if n, err := strconv.Atoi(strings.TrimPrefix(ver, base+"-")); err == nil {
			cur = n
		}
	}
	seq := cur
	if n, ok := maxHistorySeq(); ok && n > seq {
		seq = n
	}
	seq++
	gitCount := exec.Command("git", "rev-list", "--count", "HEAD", "--", ".")
	gitCount.Dir = root
	if out, err := gitCount.Output(); err == nil {
		if n, err := strconv.Atoi(strings.TrimSpace(string(out))); err == nil && n > seq {
			return base + "-" + strconv.Itoa(n)
		}
	}
	return base + "-" + strconv.Itoa(seq)
}

// maxHistorySeq 扫根目录与输出目录里的 Fntv-Plus-<数字>[-arm64].fpk（含历史 v 前缀形态），
// 返回最大 N（包挪进 release/ 后痕迹依然可追；序号与架构无关，两个架构共用一套序列）。
func maxHistorySeq() (int, bool) {
	// 兼容历史 Fntv-Plus-v123.fpk、新 Fntv-Plus-123.fpk 与架构后缀 Fntv-Plus-123-arm64.fpk
	// （发布包带点，不匹配）
	re := regexp.MustCompile(`^Fntv-Plus-(?:v)?(\d+)(?:-arm64)?\.fpk$`)
	max, ok := 0, false
	for _, dir := range []string{root, defaultOutDir()} {
		entries, err := os.ReadDir(dir)
		if err != nil {
			continue
		}
		for _, e := range entries {
			if e.IsDir() {
				continue
			}
			if m := re.FindStringSubmatch(e.Name()); m != nil {
				if n, err := strconv.Atoi(m[1]); err == nil && n > max {
					max, ok = n, true
				}
			}
		}
	}
	return max, ok
}

// writeVersion 把 version 写回 manifest（开发版流程用；发布版号存 release_version 独立键）。
func writeVersion(ver string) {
	path := filepath.Join(root, "manifest")
	data, err := os.ReadFile(path)
	if err != nil {
		return
	}
	updated := regexp.MustCompile(`(?m)^(\s*version\s*=\s*)\S+`).ReplaceAllString(string(data), "${1}"+ver)
	_ = os.WriteFile(path, []byte(updated), 0o644)
}

func manifestVersion() string {
	data, err := os.ReadFile(filepath.Join(root, "manifest"))
	if err != nil {
		return "?"
	}
	m := regexp.MustCompile(`(?m)^\s*version\s*=\s*(\S+)`).FindStringSubmatch(string(data))
	if len(m) < 2 {
		return "?"
	}
	return strings.TrimSpace(m[1])
}

func manifestGetString(key, def string) string {
	data, err := os.ReadFile(filepath.Join(root, "manifest"))
	if err != nil {
		return def
	}
	m := regexp.MustCompile(`(?m)^\s*` + key + `\s*=\s*(\S+)`).FindStringSubmatch(string(data))
	if len(m) < 2 {
		return def
	}
	return strings.TrimSpace(m[1])
}

// manifestSetString 写 manifest 的 key=value（display_name / release_version / version）。
func manifestSetString(key, value string) error {
	path := filepath.Join(root, "manifest")
	data, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	re := regexp.MustCompile(`(?m)^(` + key + `\s*=\s*)\S*`)
	if !re.Match(data) {
		return fmt.Errorf("manifest 缺少键: %s", key)
	}
	updated := re.ReplaceAllString(string(data), "${1}"+value)
	return os.WriteFile(path, []byte(updated), 0o644)
}

func copyFile(src, dst string) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
		return err
	}
	out, err := os.Create(dst)
	if err != nil {
		return err
	}
	defer out.Close()
	_, err = io.Copy(out, in)
	return err
}
