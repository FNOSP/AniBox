//go:build builtin_example

// Package secret —— builtin.example.go：内置第三方凭证的**模板**（不含任何真实密钥）。
//
// 用法：
//  1. 复制本文件为同目录的 builtin.go（真实文件已被 .gitignore 排除，绝不入库）；
//  2. 删除第一行的 `//go:build builtin_example` 构建约束；
//  3. 用 `cmd/secrettmp`（或等价的本地工具）以
//     密钥 = secret.Derive("dandanplay-builtin-v1") 生成密文，填入下面的占位值。
//
// 未生成密文 / 使用占位值时，BuiltinDandanplay() 解密失败会返回 nil，
// 应用按「无内置凭证」降级运行（设置面板仍可填自定义 AppId/Secret），不影响编译与启动。
package secret

// builtinDandanplayAppID 内置弹弹play AppId（密文占位）。
var builtinDandanplayAppID = "enc:v1:REPLACE_WITH_CIPHERTEXT"

// builtinDandanplaySecrets 内置弹弹play 应用密钥（密文占位，按优先级排列，失败自动顺延）。
var builtinDandanplaySecrets = []string{
	"enc:v1:REPLACE_WITH_CIPHERTEXT",
}
