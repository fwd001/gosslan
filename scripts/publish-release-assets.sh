#!/usr/bin/env bash
# 把一个 tag 的构建产物挂到 GitHub Release。
#
# 为什么从三份 YAML 里抽出来（2026-09-30 实测）：`build.yml` / `build-macos.yml` / `build-android.yml`
# 各带一个 `release:` 任务，同一个 tag 三条**并发**跑，而旧写法是「先查不存在 ⇒ 再创建」——
# 这个窗口里的第二家会拿到 "Release already exists" 而整步判红（⚠️ 但 2026-09-30 那次三条全红**不是**这个 ——
# 见下面「今天的第二次纠正」：真因是缺仓库上下文，这段并发窗口只是它顺手一起收掉的旧隐患）。创建那一步当时**没有**重试，
# 只有上传有；而这段逻辑抄三份本身就是一份会漂的真相。
#
# 今天的第二次纠正（v4.31.40 那次三条全红，逐 step 现读后才看清）：
# ★ 每一条 `gh release ...` 都必须显式带 `--repo`。这个 job 里**没有 actions/checkout**
#   （它只下载产物），而 `gh release create/upload` 要从 git 工作目录或参数里推出仓库上下文 ——
#   不在仓库里又不带 `--repo` 就直接失败。同一段脚本里 `gh api repos/$repo/...` 因为手写全路径
#   "看着是好的"，所以失败点被藏在下一行，CI 只留一句 "Process completed with exit code 1."。
#
# 现在的规则：
# 1. 创建是**幂等**的：已存在就复用；创建失败后**再查一次** —— 被并发的另一条建好就认它并继续上传，
#    仍然不存在才算真失败（401/403/网络坏绝不许被读成"成功"）。
# 2. 上传带退避重试，超过上限就非零退出（不 `continue-on-error`，不发半份）。
# 3. 产物目录不存在或是空的 ⇒ 直接红：宁可不发，也不发一个"看起来发好了而附件少一档"的 Release。
#
# 本地怎么验（不需要凭据、不需要 runner）：`node --test scripts/publishRelease.test.ts`
# 用一份假 `gh` 把七种结局各跑一遍（含"创建真失败必须是红"这一条）。
set -euo pipefail

assets_dir="assets"
max_attempts="${GOSSLAN_RELEASE_UPLOAD_ATTEMPTS:-5}"
sleep_base="${GOSSLAN_RELEASE_SLEEP_BASE:-5}"
gh_bin="${GH_BIN:-gh}"
tag="${GOSSLAN_RELEASE_TAG:-${GITHUB_REF_NAME:-}}"
sha="${GOSSLAN_RELEASE_SHA:-${GITHUB_SHA:-}}"
repo="${GOSSLAN_RELEASE_REPO:-${GITHUB_REPOSITORY:-}}"

while [ $# -gt 0 ]; do
  case "$1" in
    --assets) assets_dir="$2"; shift 2 ;;
    --tag) tag="$2"; shift 2 ;;
    --sha) sha="$2"; shift 2 ;;
    --repo) repo="$2"; shift 2 ;;
    --attempts) max_attempts="$2"; shift 2 ;;
    --sleep-base) sleep_base="$2"; shift 2 ;;
    *) echo "未知参数: $1（可用 --assets/--tag/--sha/--repo/--attempts/--sleep-base）" >&2; exit 2 ;;
  esac
done

if [ -z "$tag" ] || [ -z "$repo" ]; then
  echo "缺 tag 或 repo（GITHUB_REF_NAME / GITHUB_REPOSITORY 都要在）" >&2
  exit 2
fi

# 空产物必须在任何 gh 调用**之前**拦下：少一档附件这件事 v4.31.37 已经发生过一次（那次是并发抢，
# 这次如果静默发空 Release，界面上会是"发版成功、下载页少文件" —— 更难查。
if [ ! -d "$assets_dir" ]; then
  echo "没有 $assets_dir 目录 ⇒ download-artifact 一条都没取到，拒绝发空 Release" >&2
  exit 1
fi
files=()
while IFS= read -r f; do
  files+=("$f")
done < <(find "$assets_dir" -type f | sort)
if [ "${#files[@]}" -eq 0 ]; then
  echo "$assets_dir 里一个文件都没有 ⇒ 拒绝发空 Release" >&2
  exit 1
fi

release_exists() { "$gh_bin" api "repos/$repo/releases/tags/$tag" >/dev/null 2>&1; }

if release_exists; then
  echo "Release $tag 已存在 ⇒ 不重复创建，只补文件"
else
  # ★ 创建必须知道挂在哪条提交上。CI 里有 GITHUB_SHA，本地补挂没有 ⇒ 空 `--target` 会被 gh
  #   当成"找一个叫空字符串的对象"，报错还落在下一行，读起来像脚本坏了。所以在这里明确要参数。
  if [ -z "$sha" ]; then
    echo "Release $tag 不存在，而创建它必须知道挂在哪条提交 ⇒ 传 --sha（本地可用 git rev-parse $tag^{commit}）" >&2
    exit 2
  fi
  if ! create_err=$("$gh_bin" release create "$tag" --repo "$repo" --target "$sha" --generate-notes --title "$tag" 2>&1); then
    echo "创建没成功（原样贴出，方便分清是被抢先建好还是权限/网络）："
    echo "$create_err"
    # ★ 竞争窗口就在这一次复查询里收掉：后到的那条不报错，它只是"来晚了"。
    if ! release_exists; then
      echo "复查询仍说不存在 ⇒ 这不是并发，是真失败（不发半份）" >&2
      exit 1
    fi
    echo "是并发的另一条刚建好 ⇒ 认它，继续上传"
  fi
fi

attempt=0
upload_err=""          # set -u 之下先给个初值：失败分支里那句"把原因贴出来"不许自己变成 unbound
while :; do
  if upload_err=$("$gh_bin" release upload "$tag" --repo "$repo" "${files[@]}" --clobber 2>&1); then
    break
  fi
  attempt=$((attempt + 1))
  if [ "$attempt" -ge "$max_attempts" ]; then
    echo "挂了 $max_attempts 次仍失败 ⇒ 不假装成功" >&2
    echo "$upload_err" >&2
    exit 1
  fi
  echo "第 $attempt 次上传失败（${upload_err}），等 $((attempt * sleep_base))s 再试"
  sleep $((attempt * sleep_base))
done

echo "已把 ${#files[@]} 个文件挂到 ${tag}："
printf '  %s\n' "${files[@]}"
