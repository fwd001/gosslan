// 共享目录遍历与落盘命名：`ShareTreeRequest` 的目录树 + 收件侧的文件名裁决。
//
// 为什么单独一册：这一册**不碰网络也不碰会话状态**，判的是"这个路径能不能给出去"和
// "这个文件名在这台机器上要落成什么"（同名加序号、类型档位、人类可读大小）。
// 它是纯函数集，出问题只可能在这几段里。
//

// 恒等判据（与 transport 那五刀同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点恰好命中一次（本模块的锚点由 runner 沿 include! 树自动解析 ⇒ 不动 Case 的 file=）、
//   clippy `-D warnings`、`cargo fmt --check`。
// ⚠️ 搬家同批必须做的两件事：`network/mod.rs::file_src_for_guards()` 登记本册（漏了=形状守卫看不见这段生产码，假绿），
//   以及 `docs/domains.data.mjs` 的 transport/file 领域 paths（漏了=判据 D 报无主文件）。

/// 递归枚举共享目录树（限制深度 8，跳过隐藏文件）。
pub fn walk_share_dir(root: &Path) -> Vec<ShareEntry> {
    let mut out = Vec::new();
    walk(root, "", &mut out, 0);
    out
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<ShareEntry>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(file_type) = e.file_type() else {
            continue;
        };
        // 不跟随符号链接，避免共享目录枚举泄露共享根目录之外的路径。
        if file_type.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let rel_path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let is_dir = file_type.is_dir();
        let size = if is_dir {
            0
        } else {
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        };
        out.push(ShareEntry {
            name,
            path: rel_path.clone(),
            is_dir,
            size,
        });
        if is_dir {
            walk(&path, &rel_path, out, depth + 1);
        }
    }
}

/// 按文件扩展名（大小写不敏感）保守分类附件类型：`image` / `code` / `file`。
///
/// 这是纯函数，**仅依赖 basename**：FileOffer 已把 `name` 带到接收端，两端各自调用
/// 同一实现 → 分类结果天然一致，无需给文件传输协议增加字段。
///
/// 从路径提取最可靠的文件名。
///
/// Tauri Android file picker 有时把 content:// URI 转存到临时文件，
/// `Path::file_name()` 返回无扩展名的 `xxx`（比如 `478812312`），
/// 但原始路径字符串里可能仍保留着正确的扩展名。
/// 这个函数做三级 fallback：
/// 1. Path::file_name() 正常返回且有扩展名 → 直接用
/// 2. Path::file_name() 没扩展名 → 从完整 path 字符串找最后一个 `.xxx` 模式补上
/// 3. 都没有 → 返回 file_name() 的原值
pub(crate) fn derive_file_name(raw_path: &str) -> String {
    let p = Path::new(raw_path);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());

    // Case 1: file_name 已经有扩展名 → 直接返回
    if p.extension().is_some() {
        return name;
    }

    // Case 2: file_name 没扩展名，但完整路径字符串末尾有类似 .mp4 / .jpg 的后缀
    let last_dot = raw_path.rfind('.');
    let last_slash = raw_path.rfind('/').unwrap_or(0);
    if let Some(dot) = last_dot {
        if dot > last_slash && dot + 1 < raw_path.len() {
            let ext_candidate = &raw_path[dot + 1..];
            if (1..=10).contains(&ext_candidate.len())
                && ext_candidate.chars().all(|c| c.is_ascii_alphanumeric())
            {
                return format!("{name}.{ext_candidate}");
            }
        }
    }

    name
}

/// 未用 MIME 魔数嗅探的原因：那要么需要给 FileOffer/RelayFileOffer 加 kind 字段
/// （违反「不修改文件传输协议」），要么两端各自读字节嗅探（引入 sender/receiver 分歧）。
/// 任务给出的图片/代码清单本身即扩展名，扩展名判定已足够保守且确定。
///
/// ⚠️ `md`（Markdown）是**文档**而非代码，刻意排除在 `code` 之外——若把 .md 归为 code，
/// 接收端会按「代码附件」渲染成代码预览块而非文件卡片（用户明确反馈：复制 md 文件发送
/// 不应变成代码块）。
pub fn classify_file_subtype(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        // 图片：所有主流格式（含移动端 iPhone 默认 HEIC/HEIF、Android 各种、无损 BMP/TIFF/AVIF）
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "bmp" | "tiff" | "tif"
        | "avif" | "apng" | "svg" | "ico" | "raw" | "dng" => "image",
        // 视频：移动端最常见（MP4/MOV/3GP/AVI/MKV/WebM）
        "mp4" | "mov" | "m4v" | "3gp" | "3gpp" | "avi" | "mkv" | "webm" | "flv" | "wmv" => "video",
        // 音频
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" | "opus" => "audio",
        // 代码/文本（刻意排除 md/txt/log/Makefile — 用户明确反馈 md 文件发送应保持文件卡片）
        "rs" | "ts" | "tsx" | "js" | "jsx" | "vue" | "py" | "go" | "java" | "kt" | "c" | "cpp"
        | "cc" | "h" | "hpp" | "cs" | "rb" | "php" | "swift" | "scala" | "r" | "pl" | "sh"
        | "bash" | "zsh" | "fish" | "ps1" | "bat" | "toml" | "json" | "yaml" | "yml" | "xml"
        | "html" | "htm" | "css" | "scss" | "less" | "sql" | "ini" | "cfg" | "conf" => "code",
        _ => "file",
    }
}

/// 避免重名：`a.txt` -> `a (1).txt`
pub(crate) fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let base = dir.join(name);
    if !base.exists() {
        return base;
    }
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = base
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    for i in 1..1000 {
        let cand = if ext.is_empty() {
            dir.join(format!("{stem} ({i})"))
        } else {
            dir.join(format!("{stem} ({i}).{ext}"))
        };
        if !cand.exists() {
            return cand;
        }
    }
    // 同名文件已达 999 个（异常）：退回随机后缀。
    // 旧实现在此直接 `return base`，而 base 必定已存在 → 静默覆盖用户已有文件。
    for _ in 0..32 {
        let token = STANDARD.encode(crypto::random_key());
        let cand = if ext.is_empty() {
            dir.join(format!("{stem}-{}", &token[..8]))
        } else {
            dir.join(format!("{stem}-{}.{ext}", &token[..8]))
        };
        if !cand.exists() {
            return cand;
        }
    }
    // 32 次随机后缀仍冲突：用完整随机串兜底（实际不可能发生）
    let token = STANDARD.encode(crypto::random_key());
    if ext.is_empty() {
        dir.join(format!("{stem}-{token}"))
    } else {
        dir.join(format!("{stem}-{token}.{ext}"))
    }
}

/// 文件名来自远端协议，必须只允许 basename，避免 `../` / Windows `\\` 穿越下载目录。
pub(crate) fn safe_file_name(name: &str) -> Option<String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('\0') {
        return None;
    }
    if name.contains('/') || name.contains('\\') {
        return None;
    }
    Some(name.to_string())
}

/// `transfer_id` 同样来自远端协议，而且**会被直接拼进落盘路径**（`{transfer_id}.part`）——
/// 必须和 `safe_file_name` 同级校验，否则一个 `../../../../Users/me/Documents/x` 就能逃出
/// 下载目录，而 `File::create` 会**创建或截断**目标文件（内容由对端控制，
/// `Path::join` 遇到绝对路径还会整体替换前缀）。失败收尾路径同样会 `remove_file` 它。
///
/// 白名单而非黑名单：只接受 UUID / 测试用的连字符短 id 形态（`[A-Za-z0-9_-]{1,64}`）。
/// 生产端的 transfer_id 一律是 `Uuid::new_v4().to_string()`。
pub(crate) fn safe_transfer_id(id: &str) -> Option<String> {
    if id.is_empty() || id.len() > 64 {
        return None;
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    Some(id.to_string())
}

/// 人类可读的文件大小。
#[allow(dead_code)]
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}
