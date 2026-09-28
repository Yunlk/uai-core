#!/usr/bin/env bash
# 扫描「将要进仓库的文件」里有没有真实账号数据 / 凭据。
#
# 存在的理由：2026-09-28 发现 README 的 `uai whoami` 示例抄了真实输出里的姓，
# 而 src/crypto.rs 的两个测试从 v0.1.0 起就把**真实密码与真实手机号**当夹具，
# 一路随每个 tag 发布出去。两者都违反 AGENTS.md 硬规则 1 / 2。
# 人眼会漏，所以把这条规则变成能失败的一步。
#
# 设计取向：**宁可少报，不可乱报**。只认高信号、几乎不可能是误报的形状——
# 因为一个天天误报的检查，最后一定会被 `|| true` 掉，等于没有。
# 已知的占位符（12345678901、20000000001、1111111111111111111 等）一律放行。
#
# 用法：
#   bash scripts/pii-scan.sh            # 扫工作区（CI 用这个）
#   bash scripts/pii-scan.sh --staged   # 只扫暂存区（提交前用）
set -uo pipefail

mode="${1:-worktree}"

files=()
if [ "$mode" = "--staged" ]; then
  while IFS= read -r f; do
    [ -n "$f" ] && [ -f "$f" ] && files+=("$f")
  done < <(git diff --cached --name-only --diff-filter=ACMR)
else
  # 工作区模式要连**未跟踪但没被 ignore** 的新文件一起扫——否则新建一个
  # 满是真实数据的文件、还没 git add，就会扫不到（实测踩过这个坑）。
  while IFS= read -r f; do
    [ -n "$f" ] && files+=("$f")
  done < <(git ls-files --cached --others --exclude-standard)
fi
if [ "${#files[@]}" -eq 0 ]; then
  echo "pii-scan: 没有要扫的文件"
  exit 0
fi

# 明确放行：本脚本自己写了正则，会自命中；Cargo.lock 是 cargo 生成的、
# 里面的长数字来自依赖的校验和，不是账号数据。
ignore=('scripts/pii-scan.sh' 'Cargo.lock')

hits=0
report() { printf '  %s:%s: %s\n' "$1" "$2" "$3"; hits=$((hits + 1)); }

# 放行名单：都是仓库里既有的占位符约定，或明显不是真号的号段。
#   13800000000 / 17000000000        测试夹具（末 8 位全 0）
#   12345678901 / 1234567890x        教科书占位符
#   1111111111111111111 / 20000000xxx 占位 ID
allow_number() {
  case "$1" in
    13800000000|17000000000|12345678901|12345678902|12345678903) return 0 ;;
    *00000000|*11111111|*12345678) return 0 ;;
  esac
  return 1
}

for f in "${files[@]}"; do
  skip=0
  for i in "${ignore[@]}"; do [ "$f" = "$i" ] && skip=1; done
  [ "$skip" = 1 ] && continue
  # 只扫文本文件；二进制（图片、arsc 等）跳过
  grep -Iq . "$f" 2>/dev/null || continue

  # 1) 会话缓存被实体化提交：里面有门户 JWT 与 open_id，等于账号钥匙
  while IFS=: read -r line text; do
    [ -n "${line:-}" ] || continue
    report "$f" "$line" "疑似会话缓存字段/令牌：$(printf '%s' "$text" | cut -c1-64)"
  done < <(grep -nE '"(portal_token|annotator_token|app_user_id)"[[:space:]]*:' "$f" 2>/dev/null)

  # 2) 真实手机号（11 位）。占位符由 allow_number 放行。
  while IFS=: read -r line text; do
    [ -n "${line:-}" ] || continue
    for n in $(printf '%s' "$text" | grep -oE '1[3-9][0-9]{9}'); do
      allow_number "$n" || report "$f" "$line" "疑似真实手机号：$n"
    done
  done < <(grep -nE '1[3-9][0-9]{9}' "$f" 2>/dev/null)

  # 3) 形如 user/pass/secret/token = "很长的串" 的硬编码凭据。
  #    只认「同时含字母和数字、长度 ≥ 24、且不带 example/placeholder/test」
  #    的字符串，避免把 UUID、base64 常量、示例值全报出来。
  while IFS=: read -r line text; do
    [ -n "${line:-}" ] || continue
    for v in $(printf '%s' "$text" | grep -oE '"[A-Za-z0-9_@#$%^&*!.-]{24,}"'); do
      inner="${v%\"}"; inner="${inner#\"}"
      case "$inner" in
        *example*|*Example*|*placeholder*|*PLACEHOLDER*|*test*|*Test*|*dummy*|*Dummy*) continue ;;
      esac
      printf '%s' "$inner" | grep -qE '[A-Za-z]' || continue
      printf '%s' "$inner" | grep -qE '[0-9]' || continue
      report "$f" "$line" "疑似硬编码凭据（长度 ${#inner}）：$(printf '%s' "$inner" | cut -c1-32)…"
    done
  done < <(grep -niE '(^|[^A-Za-z])(user|username|pass|passwd|password|secret|token|apikey|api_key|llm_sk)[^A-Za-z]{0,3}[:=]' "$f" 2>/dev/null)

  # 4) 凭据写成环境变量赋值（README/脚本里手滑贴真实值最典型的形状）
  while IFS=: read -r line text; do
    [ -n "${line:-}" ] || continue
    case "$text" in *'<'*|*example*|*Example*|*占位*|*手机号*|*密码*|*your*|*YOUR*) continue ;; esac
    report "$f" "$line" "疑似真实凭据赋值：$(printf '%s' "$text" | cut -c1-56)"
  done < <(grep -nE '(UAI_USER|UAI_PASS|UAI_PHONE|password|passwd)[[:space:]]*=[[:space:]]*["'"'"'][^"'"'"']{6,}' "$f" 2>/dev/null)

  # 5) 真名：`uai whoami` 的输出里会带真名，抄示例最容易漏
  #    （2026-09-28 就是这么把姓写进 README 的）。
  #    只认 .md/.txt/.html 里的输出形状，且跳过含 `{}` 的格式串——
  #    src/session.rs 里那句 `"{}（门户令牌 {}，open_id {}）"` 是格式模板，
  #    不是真实输出，不该报。
  case "$f" in
    *.md|*.txt|*.html)
      while IFS=: read -r line text; do
        [ -n "${line:-}" ] || continue
        case "$text" in *'{'*'}'*) continue ;; esac
        # 放行明写的占位名
        case "$text" in
          *示例用户*|*测试用户*|*占位*|*example*|*Example*|*demo*|*Demo*|*test*|*Test*) continue ;;
        esac
        report "$f" "$line" "疑似 whoami 真实输出（带姓名）：$(printf '%s' "$text" | cut -c1-48)"
      done < <(grep -nE '（门户令牌' "$f" 2>/dev/null)
      ;;
  esac
done

if [ "$hits" -gt 0 ]; then
  cat >&2 <<'EOF'

✗ pii-scan 失败：上面这些位置疑似真实账号数据或凭据。

按 AGENTS.md 硬规则 1 / 2：
  - 代码、测试、文档、示例一律用占位符（1111111111111111111、
    course-v2:example…、20000000001、13800000000）
  - 真实 classId / courseId / courseResourceId / 实例 ID / open_id /
    school / 姓名 / 手机号 / 密码，都不得出现在仓库里
  - 会话缓存（uai-session.json）永不进仓库

修法：换成占位符并重新提交。若确有无法替换的夹具，把它加进
scripts/pii-scan.sh 顶部的 ignore / allow_number，并在注释里写清理由。
EOF
  exit 1
fi

echo "✓ pii-scan 通过：${#files[@]} 个文件，未发现真实账号数据"
