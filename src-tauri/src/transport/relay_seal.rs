//! 中继电路的**记录层封装**（公网中继的客户端一侧）。
//!
//! ## 为什么必须有这一层
//! Gosslan 的正文一直是端到端加密的，但**帧的其余部分是明文 JSON**：`type` 判别符、
//! `device_id`、`from`/`to`、昵称、群名册、文件名与大小、已读回执全部可读
//! （帧定义见 `protocol.rs`，线格式见 `transport/tcp.rs` 的 `4B 长度 + JSON`）。
//!
//! 在局域网里这不是问题 —— 路径本身可信。但"任意一台能连上的中转服务器"不是可信路径：
//! 它不仅能读，还能**注入**（`Message::ChatMessage` 直连帧没有信封签名，只靠
//! 「链路对端 == from」兜，见 INV-P21），而伪造的 `Ack` 会让发送方删掉 outbox 行。
//! 所以走公网中继的电路**必须**在帧流外面再套一层带认证的记录层，否则
//! "服务器只管组网、读不到也存不了"这句话只成立一半。
//!
//! ## 这一层买到什么
//! - 正文：本来就 E2EE，不变。
//! - 元数据（设备 ID / 昵称 / 群名册 / 文件名 / 帧类型 / 已读回执）：✅ 进密文，中继读不到。
//! - 中继注入任意帧：✅ AEAD 标签 + 严格递增计数器，注入即断链。
//! - 中继重放早先的记录：✅ 计数器只增，重放即断链。
//! - 中继冒充某个好友：✅ 协商用 **friends 表绑定的 Ed25519 公钥**验签，不认自报公钥。
//! - 帧长度、收发时刻、总量：❌ 仍可见（不做填充；长度分档是已知残留）。
//! - 中继主动断开：❌ 拦不住（那是它的工作方式，不是攻击）。
//!
//! ## 为什么不用 TLS
//! 全仓零 TLS 依赖，而这里的身份**已经**是每台设备长期持有的 Ed25519 公钥（加好友时绑定）。
//! 用已有的密钥 + 已有的原语（X25519 + ChaCha20-Poly1305）比引入一套并行信任体系更小，
//! 而且**不改 Gosslan 的线格式**：帧本身一个字节都没动，老版本设备不受任何影响。
//!
//! ## 协商线（电路建立后、`Hello` 之前，双向同时发 ⇒ 不会死锁）
//! ```text
//! GSRW1 <role A|B> <b64 临时 X25519> <b64 签名>\n
//! 签名覆盖 ("gosslan-relay-wrap-v1", 我的 device_id, 期望的对端 device_id,
//!           通道哈希, role, 临时公钥)
//! ```
//! 协商线上**既没有 device_id 也没有长期公钥**：身份由签名"证明"。
//! 少带长期公钥不只是省字节 —— 中继若拿到它，就能反算出以后每一天的通道哈希，
//! "按天换通道"随之失效。而校验方本来就有 friends 绑定值，不需要对方再报一次。
//! `期望的对端 device_id` 进签名材料是刻意的 —— 把"这条协商是给谁的"钉死，
//! 中继把我的协商行转送给第三人也会因对不上而失败。

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use ed25519_dalek::{Signer, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::io;
use x25519_dalek::{EphemeralSecret, PublicKey};

use crate::protocol::MAX_FRAME;

/// 通道哈希的域前缀。
pub const CHANNEL_PREFIX: &[u8] = b"gosslan-relay-ch-v1";
/// 协商签名的域前缀。
pub const WRAP_PREFIX: &[u8] = b"gosslan-relay-wrap-v1";
/// 记录密钥派生的域前缀。
pub const RECORD_PREFIX: &[u8] = b"gosslan-relay-record-v1";
/// 协商线首字段。
pub const WRAP_MAGIC: &str = "GSRW1";
/// 协商线最大字节数（实测约 190B，留余量）。
pub const WRAP_LINE_MAX: usize = 512;

/// 记录明文里额外占的字节数（8 字节递增计数器）。
const CTR_LEN: usize = 8;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
/// 单条记录相对明文的固定开销（nonce + 标签）。
pub const RECORD_OVERHEAD: usize = NONCE_LEN + TAG_LEN;
/// 内层帧的长度前缀宽度（与 `transport/tcp.rs` 一致）。
const INNER_LEN_PREFIX: usize = 4;
/// 加密后待写出字节的硬上限：超过即判这条链路有问题（正常永远到不了，
/// 因为写侧每次只多一帧，而读侧会立刻被 socket 的 Pending 卡住）。
const OUT_BACKLOG_MAX: usize = MAX_FRAME + RECORD_OVERHEAD + 8;

/// 一对好友在中继上的**通道哈希**：中继只看到这一串摘要，不知道是谁和谁。
///
/// `epoch_day` = UTC 天数。带上它不是为了 secrecy，而是不让"这一对设备"变成
/// 永久可关联的假名。代价是换天那一刻两侧可能各算一天 —— 客户端每 10s 一轮重拨，
/// 几分钟内自愈，不需要额外的边界协商代码。
pub fn channel_hash(
    my_ed25519_b64: &str,
    peer_ed25519_b64: &str,
    my_device_id: &str,
    peer_device_id: &str,
    epoch_day: u64,
) -> [u8; 32] {
    // 排序判据用 device_id（与 `crypto::safety_number` 同一选择：恒 ASCII 字典序，
    // 两端结果必然一致），公钥跟着各自的 ID 走。
    let (first_id, first_pk, second_id, second_pk) = if my_device_id <= peer_device_id {
        (
            my_device_id,
            my_ed25519_b64,
            peer_device_id,
            peer_ed25519_b64,
        )
    } else {
        (
            peer_device_id,
            peer_ed25519_b64,
            my_device_id,
            my_ed25519_b64,
        )
    };
    let mut h = Sha256::new();
    h.update(CHANNEL_PREFIX);
    for field in [first_id, first_pk, second_id, second_pk] {
        h.update(field.as_bytes());
        h.update([0u8]); // 分隔符：消除 "ab"+"c" 与 "a"+"bc" 的拼接歧义
    }
    h.update(epoch_day.to_be_bytes());
    h.finalize().into()
}

/// 通道哈希的十六进制表示（中继协议里的通道字段）。
pub fn channel_hex(ch: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in ch {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 解析十六进制通道字段；长度或字符非法返回 `None`。
pub fn parse_channel_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// 协商角色：由 `device_id` 字典序定，**两端各自算出同一个值**。
///
/// 为什么不用"谁先连上中继"当判据：电路是对称的，两端各自拨、谁先到无所谓。
/// 若 role 由到达顺序决定，两端就会算出不同的方向密钥 —— 表现是"静默互相解不开"，
/// 这是最难查的一类故障。用 ID 字典序则与到达顺序、时钟、重启全都无关。
pub fn wrap_role(my_device_id: &str, peer_device_id: &str) -> &'static str {
    if my_device_id <= peer_device_id {
        "A"
    } else {
        "B"
    }
}

/// 协商签名材料（与 `protocol::hello_signing_bytes` 同形：serde_json 定长元组）。
pub fn wrap_signing_bytes(
    my_device_id: &str,
    peer_device_id: &str,
    channel_hex: &str,
    role: &str,
    eph_pub_b64: &str,
) -> Vec<u8> {
    serde_json::to_vec(&(
        WRAP_PREFIX,
        my_device_id,
        peer_device_id,
        channel_hex,
        role,
        eph_pub_b64,
    ))
    .expect("元组序列化不会失败")
}

/// 临时 X25519 密钥对。
///
/// `secret` 是消费型的（`accept_wrap_offer` 拿走它）：编译器保证一把临时钥只用于一条电路。
pub struct EphKeypair {
    pub secret: EphemeralSecret,
    pub public_b64: String,
}

pub fn generate_eph_keypair() -> EphKeypair {
    let secret = EphemeralSecret::random_from_rng(rand_core::OsRng);
    EphKeypair {
        public_b64: STANDARD.encode(PublicKey::from(&secret).as_bytes()),
        secret,
    }
}

/// 我方发出（也是解析对端发来）的协商线。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrapOffer {
    pub role: String,
    pub eph_pub_b64: String,
    pub sig_b64: String,
}

impl WrapOffer {
    /// 线上形式：`GSRW1 <role> <epk> <sig>\n`（**不带**长期公钥，见文件头）。
    pub fn to_line(&self) -> String {
        format!(
            "{} {} {} {}\n",
            WRAP_MAGIC, self.role, self.eph_pub_b64, self.sig_b64
        )
    }

    /// 解析对端协商线。**只**做语法与长度闸门，验签在 `accept_wrap_offer`。
    pub fn parse_line(line: &str) -> Option<Self> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.len() > WRAP_LINE_MAX {
            return None;
        }
        let mut it = line.split(' ');
        if it.next()? != WRAP_MAGIC {
            return None;
        }
        let role = it.next()?;
        if role != "A" && role != "B" {
            return None;
        }
        let eph_pub_b64 = it.next()?;
        let sig_b64 = it.next()?;
        if it.next().is_some() {
            return None; // 多余字段（含早期稿的 ed25519 字段）：判非法，不放宽
        }
        Some(Self {
            role: role.to_string(),
            eph_pub_b64: eph_pub_b64.to_string(),
            sig_b64: sig_b64.to_string(),
        })
    }
}

/// 生成我方协商线。签名材料里带上**我自己的** device_id 与我要找的人，
/// 对端反过来核对，于是两侧互相钉死了身份。
pub fn build_wrap_offer(
    my_device_id: &str,
    peer_device_id: &str,
    channel_hex: &str,
    signing_key: &ed25519_dalek::SigningKey,
    eph: &EphKeypair,
) -> WrapOffer {
    let role = wrap_role(my_device_id, peer_device_id).to_string();
    let data = wrap_signing_bytes(
        my_device_id,
        peer_device_id,
        channel_hex,
        &role,
        &eph.public_b64,
    );
    WrapOffer {
        role,
        eph_pub_b64: eph.public_b64.clone(),
        sig_b64: STANDARD.encode(signing_key.sign(&data).to_bytes()),
    }
}

/// 一次成功协商的产物。
pub struct WrappedSession {
    /// 我 → 对端 的记录密钥。
    pub out_key: [u8; 32],
    /// 对端 → 我 的记录密钥。
    pub in_key: [u8; 32],
}

/// 校验对端协商线并派生双向密钥。
///
/// `expected_ed25519_b64` **必须来自 friends 表的绑定值**（或已 `keys_verified` 的 peers 值），
/// 不能来自协商线自报 —— 那正是本层要防的事。绑定值不符、签名不符、通道不符、
/// role 与 ID 字典序不符，一律 `None`，调用方断链；**没有明文兜底**（§19 Crypto Rules）。
pub fn accept_wrap_offer(
    offer: &WrapOffer,
    my_device_id: &str,
    peer_device_id: &str,
    expected_ed25519_b64: &str,
    channel_hex: &str,
    my_secret: EphemeralSecret,
) -> Option<WrappedSession> {
    // role 必须由 ID 字典序唯一决定：不符说明对端以为在和别人说话（或中继串了线）。
    if offer.role != wrap_role(peer_device_id, my_device_id) {
        return None;
    }
    // 验签直接用**本地绑定值**：协商线上不带长期公钥，也不该带（见文件头）。
    let pk: [u8; 32] = decode_fixed::<32>(expected_ed25519_b64)?;
    let vk = VerifyingKey::from_bytes(&pk).ok()?;
    let sig: [u8; 64] = decode_fixed::<64>(&offer.sig_b64)?;
    let data = wrap_signing_bytes(
        peer_device_id,
        my_device_id,
        channel_hex,
        &offer.role,
        &offer.eph_pub_b64,
    );
    if vk
        .verify(&data, &ed25519_dalek::Signature::from_bytes(&sig))
        .is_err()
    {
        return None;
    }
    let eph: [u8; 32] = decode_fixed::<32>(&offer.eph_pub_b64)?;
    let dh = my_secret.diffie_hellman(&PublicKey::from(eph));
    let (a2b, b2a) = record_keys(dh.as_bytes(), channel_hex);
    let (out_key, in_key) = if offer.role == "A" {
        (b2a, a2b) // 对端是 A ⇒ 我是 B：我发 B→A，我收 A→B
    } else {
        (a2b, b2a)
    };
    Some(WrappedSession {
        out_key,
        in_key,
    })
}

/// 双向两条独立密钥（方向不互用 ⇒ 反射攻击的第一道闸门）。
fn record_keys(dh: &[u8; 32], channel_hex: &str) -> ([u8; 32], [u8; 32]) {
    let derive = |dir: &[u8]| {
        let mut h = Sha256::new();
        h.update(RECORD_PREFIX);
        h.update(dh);
        h.update(channel_hex.as_bytes());
        h.update([0u8]);
        h.update(dir);
        h.finalize().into()
    };
    (derive(b"a2b"), derive(b"b2a"))
}

fn decode_fixed<const N: usize>(s: &str) -> Option<[u8; N]> {
    STANDARD.decode(s).ok()?.try_into().ok()
}

// ───────────────────────────── 记录层 ─────────────────────────────

/// 出站记录盒：`seal(计数器 || 内层帧)`。
///
/// 计数器为什么进明文而不是当 AAD：`crypto.rs` 的 AEAD 封装全仓不传 AAD，
/// 在这里单开一条 AAD 路径会造出"同一原语两种用法"的第二份真相。进明文同样被标签覆盖，
/// 且接收端判严格递增 —— 效果一致，用法一致。
///
/// nonce 用 `4B 零 || 计数器` 而不是随机：同一把密钥下 nonce 由构造保证不重复，
/// 也就不依赖 96-bit 随机数的生日界。
#[derive(Clone)]
pub struct RecordSeal {
    key: [u8; 32],
    ctr: u64,
}

impl RecordSeal {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key, ctr: 0 }
    }

    /// 封一条内层帧（`4B 长度 + JSON` 整段）。返回 `nonce || 密文 || 标签`。
    pub fn seal_frame(&mut self, frame: &[u8]) -> Option<Vec<u8>> {
        let ctr = self.ctr.checked_add(1)?;
        let mut pt = Vec::with_capacity(CTR_LEN + frame.len());
        pt.extend_from_slice(&ctr.to_be_bytes());
        pt.extend_from_slice(frame);

        let mut nonce = [0u8; NONCE_LEN];
        nonce[4..].copy_from_slice(&ctr.to_be_bytes());
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        let ct = cipher
            .encrypt(Nonce::from_slice(&nonce), pt.as_slice())
            .ok()?;
        self.ctr = ctr;

        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Some(out)
    }
}

/// 入站记录盒：**要求计数器严格等于 `expect + 1`**。
///
/// 缺口（少一条）同样判失败：内层是有序字节流，少一条就意味着有东西在改动这条流。
/// 与之"兼容"没有意义，正确反应是断链、让选路回落别的链路。
#[derive(Clone)]
pub struct RecordOpen {
    key: [u8; 32],
    expect: u64,
}

impl RecordOpen {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key, expect: 0 }
    }

    pub fn open_record(&mut self, record: &[u8]) -> Option<Vec<u8>> {
        if record.len() < NONCE_LEN + TAG_LEN + CTR_LEN {
            return None;
        }
        let (nonce, ct) = record.split_at(NONCE_LEN);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        let pt = cipher.decrypt(Nonce::from_slice(nonce), ct).ok()?;
        let (ctr_bytes, frame) = pt.split_at(CTR_LEN);
        let ctr = u64::from_be_bytes(ctr_bytes.try_into().ok()?);
        if ctr != self.expect + 1 {
            return None;
        }
        self.expect = ctr;
        Some(frame.to_vec())
    }
}

// ────────────────────── 字节流适配（给 AsyncRead/AsyncWrite 用）──────────────────────

/// 出站管道：把「`4B 长度 || 帧`」的明文字节流封成「`4B 长度 || 记录`」的密文字节流。
///
/// 为什么要自己做内层帧边界：`write_bytes` 是两次 `write_all`（先 4 字节头、再载荷），
/// 到 `poll_write` 这里的切分位置**完全不可预期**（可能一次给 3 字节）。所以要按
/// 「先攒够 4 字节头 → 知道总长 → 攒够载荷 → 封一条记录」来走。
pub struct SealPipe {
    seal: RecordSeal,
    head: [u8; INNER_LEN_PREFIX],
    head_len: usize,
    /// 载荷还需要收集的字节数；`None` = 还在收头部。
    body_need: Option<usize>,
    frame: Vec<u8>,
    out: Vec<u8>,
    out_pos: usize,
    /// 本次 `poll_write` 的字节是否已被吸收（socket 不可写时要原样重试，不能吞两遍）。
    staged: bool,
}

impl SealPipe {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            seal: RecordSeal::new(key),
            head: [0u8; INNER_LEN_PREFIX],
            head_len: 0,
            body_need: None,
            frame: Vec::new(),
            out: Vec::new(),
            out_pos: 0,
            staged: false,
        }
    }

    pub fn clear_staged(&mut self) {
        self.staged = false;
    }

    pub fn has_out(&self) -> bool {
        self.out_pos < self.out.len()
    }

    pub fn out_slice(&self) -> &[u8] {
        &self.out[self.out_pos..]
    }

    pub fn advance_out(&mut self, n: usize) {
        self.out_pos += n;
        if self.out_pos >= self.out.len() {
            self.out.clear();
            self.out_pos = 0;
        }
    }

    /// 吸收调用方写来的字节（切分任意）。攒完整帧就地加密进 `out`。
    pub fn absorb(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.staged {
            return Ok(()); // 上一次调用已经吞过，这次只是重试写出
        }
        self.staged = true;
        let mut i = 0usize;
        while i < buf.len() {
            if self.body_need.is_none() {
                let take = (INNER_LEN_PREFIX - self.head_len).min(buf.len() - i);
                self.head[self.head_len..self.head_len + take].copy_from_slice(&buf[i..i + take]);
                self.head_len += take;
                i += take;
                if self.head_len == INNER_LEN_PREFIX {
                    let len = u32::from_be_bytes(self.head) as usize;
                    if len == 0 || len > MAX_FRAME {
                        return Err(invalid("内层帧长度非法"));
                    }
                    self.frame.clear();
                    self.frame.extend_from_slice(&self.head);
                    self.head_len = 0;
                    self.body_need = Some(len);
                }
                continue;
            }
            let need = self.body_need.unwrap();
            let take = need.min(buf.len() - i);
            self.frame.extend_from_slice(&buf[i..i + take]);
            self.body_need = Some(need - take);
            i += take;
            if self.body_need.unwrap() == 0 {
                self.body_need = None;
                let record = self
                    .seal
                    .seal_frame(&self.frame)
                    .ok_or_else(|| invalid("记录封装失败"))?;
                let mut hdr = (record.len() as u32).to_be_bytes().to_vec();
                hdr.extend_from_slice(&record);
                self.out.extend_from_slice(&hdr);
                self.frame.clear();
                if self.out.len() - self.out_pos > OUT_BACKLOG_MAX {
                    return Err(invalid("待写出密文积压超限"));
                }
            }
        }
        Ok(())
    }
}

/// 入站管道：把「`4B 长度 || 记录`」的密文字节流解成「`4B 长度 || 帧`」的明文字节流。
pub struct OpenPipe {
    open: RecordOpen,
    head: [u8; INNER_LEN_PREFIX],
    head_len: usize,
    body_need: Option<usize>,
    record: Vec<u8>,
    plain: VecDeque<u8>,
}

impl OpenPipe {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            open: RecordOpen::new(key),
            head: [0u8; INNER_LEN_PREFIX],
            head_len: 0,
            body_need: None,
            record: Vec::new(),
            plain: VecDeque::new(),
        }
    }

    /// 是否收到了半条记录（对端在记录中间断开）。
    ///
    /// 有序字节流上"半条记录 + EOF"只可能是被截断，不能当正常收尾 —— 报出来，
    /// 让上层断链而不是把残缺帧交给 `decode_frame`。
    pub fn has_partial(&self) -> bool {
        self.head_len != 0 || self.body_need.is_some() || !self.record.is_empty()
    }

    pub fn take_plain(&mut self, dst: &mut [u8]) -> usize {
        let mut n = 0usize;
        while n < dst.len() {
            match self.plain.pop_front() {
                Some(b) => {
                    dst[n] = b;
                    n += 1;
                }
                None => break,
            }
        }
        n
    }

    /// 吸收从 socket 读到的密文字节；攒齐一条记录就地解密，明文字节入队。
    pub fn absorb_encrypted(&mut self, chunk: &[u8]) -> io::Result<()> {
        let mut i = 0usize;
        while i < chunk.len() {
            if self.body_need.is_none() {
                let take = (INNER_LEN_PREFIX - self.head_len).min(chunk.len() - i);
                self.head[self.head_len..self.head_len + take].copy_from_slice(&chunk[i..i + take]);
                self.head_len += take;
                i += take;
                if self.head_len == INNER_LEN_PREFIX {
                    let len = u32::from_be_bytes(self.head) as usize;
                    // 记录 = nonce + 密文 + 标签，且明文里至少还有 8 字节计数器 + 4 字节内层头
                    let min = NONCE_LEN + TAG_LEN + CTR_LEN + INNER_LEN_PREFIX;
                    if len < min || len > MAX_FRAME + RECORD_OVERHEAD {
                        return Err(invalid("记录长度非法"));
                    }
                    self.record.clear();
                    self.head_len = 0;
                    self.body_need = Some(len);
                }
                continue;
            }
            let need = self.body_need.unwrap();
            let take = need.min(chunk.len() - i);
            self.record.extend_from_slice(&chunk[i..i + take]);
            self.body_need = Some(need - take);
            i += take;
            if self.body_need.unwrap() == 0 {
                self.body_need = None;
                let frame = self
                    .open
                    .open_record(&self.record)
                    .ok_or_else(|| invalid("记录认证失败或计数器不连续"))?;
                self.plain.extend(frame.iter().copied());
                self.record.clear();
            }
        }
        Ok(())
    }
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::tcp::{read_bytes, write_bytes};
    use tokio::io::AsyncWriteExt;

    fn ident() -> (ed25519_dalek::SigningKey, String) {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng);
        (sk.clone(), STANDARD.encode(sk.verifying_key().to_bytes()))
    }

    /// 通道哈希必须**两端各自算出同一个值** —— 否则中继永远配不上对，
    /// 而且这个失败在两端日志里都看起来"正常"。
    #[test]
    fn channel_hash_is_symmetric() {
        let (_, pa) = ident();
        let (_, pb) = ident();
        let day = 20_000u64;
        let from_a = channel_hash(&pa, &pb, "gosslan-aaa", "gosslan-bbb", day);
        let from_b = channel_hash(&pb, &pa, "gosslan-bbb", "gosslan-aaa", day);
        assert_eq!(from_a, from_b, "两端必须算出同一个通道");
        assert_ne!(
            from_a,
            channel_hash(&pa, &pb, "gosslan-aaa", "gosslan-bbb", day + 1),
            "换天必须换通道（否则是永久假名）"
        );
        assert_ne!(
            from_a,
            channel_hash(&pa, &pb, "gosslan-aaa", "gosslan-ccc", day)
        );
        // 公钥换掉（同 ID 冒充）也要变：这条会抓住"公钥没进哈希"的实现错误
        let (_, pc) = ident();
        assert_ne!(
            from_a,
            channel_hash(&pc, &pb, "gosslan-aaa", "gosslan-bbb", day)
        );
    }

    #[test]
    fn channel_hash_separates_fields() {
        let (_, pk) = ident();
        let x = channel_hash(&pk, &pk, "ab", "c", 1);
        let y = channel_hash(&pk, &pk, "a", "bc", 1);
        assert_ne!(x, y, "拼接歧义必须由分隔符消除");
    }

    #[test]
    fn role_is_ordered_and_symmetric() {
        assert_eq!(wrap_role("aaa", "bbb"), "A");
        assert_eq!(wrap_role("bbb", "aaa"), "B");
        assert_eq!(wrap_role("x", "x"), "A");
    }

    #[test]
    fn channel_hex_roundtrip() {
        let (_, pk) = ident();
        let ch = channel_hash(&pk, &pk, "a", "b", 7);
        let hex = channel_hex(&ch);
        assert_eq!(hex.len(), 64);
        assert_eq!(parse_channel_hex(&hex).unwrap(), ch);
        assert!(parse_channel_hex(&hex[..63]).is_none());
        assert!(parse_channel_hex(&"z".repeat(64)).is_none());
    }

    /// 完整协商往返：双方各自得到**互换**的方向密钥。
    #[test]
    fn wrap_handshake_produces_swapped_direction_keys() {
        let (sk_a, pk_a) = ident();
        let (sk_b, pk_b) = ident();
        let (id_a, id_b) = ("gosslan-aaa", "gosslan-bbb");
        let chx = channel_hex(&channel_hash(&pk_a, &pk_b, id_a, id_b, 1));

        let kp_a = generate_eph_keypair();
        let kp_b = generate_eph_keypair();
        let off_a = build_wrap_offer(id_a, id_b, &chx, &sk_a, &kp_a);
        let off_b = build_wrap_offer(id_b, id_a, &chx, &sk_b, &kp_b);
        assert_eq!(off_a.role, "A");
        assert_eq!(off_b.role, "B");

        let sa = accept_wrap_offer(&off_b, id_a, id_b, &pk_b, &chx, kp_a.secret).unwrap();
        let sb = accept_wrap_offer(&off_a, id_b, id_a, &pk_a, &chx, kp_b.secret).unwrap();
        assert_eq!(sa.out_key, sb.in_key, "A 的发 = B 的收");
        assert_eq!(sa.in_key, sb.out_key, "A 的收 = B 的发");
        assert_ne!(sa.out_key, sa.in_key, "两个方向不得共用密钥（反射闸门）");
    }

    #[test]
    fn wrap_line_roundtrip_and_syntax_gate() {
        let (sk, pk) = ident();
        let kp = generate_eph_keypair();
        let off = build_wrap_offer("gosslan-a", "gosslan-b", "aabb", &sk, &kp);
        let line = off.to_line();
        let back = WrapOffer::parse_line(line.trim_end()).unwrap();
        assert_eq!(back, off, "写出→解析必须恒等");
        assert!(WrapOffer::parse_line(&line.replace(WRAP_MAGIC, "XXXX")).is_none());
        assert!(WrapOffer::parse_line(&format!("{} A B C D", WRAP_MAGIC)).is_none());
        assert!(
            WrapOffer::parse_line(&format!("{} X B C", WRAP_MAGIC)).is_none(),
            "role 只认 A/B"
        );
        assert!(WrapOffer::parse_line(&"q".repeat(WRAP_LINE_MAX + 1)).is_none());
        assert!(WrapOffer::parse_line("").is_none());
        // 关键隐私判据：协商线上既不得出现 device_id，也**不得出现长期公钥** ——
        // 后者一旦上线，中继就能反算任意一天的通道哈希，按天轮换白做。
        assert!(!line.contains("gosslan-a"), "协商线泄漏 device_id");
        assert!(!line.contains(&pk), "协商线泄漏长期公钥");
        assert_eq!(line.trim_end().split(' ').count(), 4, "线上只有 4 个字段");
    }

    /// 冒充检测：来线公钥 ≠ friends 绑定值 ⇒ 必须失败。这条是本层存在的全部理由。
    #[test]
    fn wrap_rejects_unbound_peer_key() {
        let (_, pk_bound) = ident();
        let (sk_evil, _) = ident();
        let (id_a, id_b) = ("gosslan-aaa", "gosslan-bbb");
        let chx = channel_hex(&channel_hash(&pk_bound, &pk_bound, id_a, id_b, 1));
        let kp_a = generate_eph_keypair();
        let off_evil = build_wrap_offer(id_b, id_a, &chx, &sk_evil, &generate_eph_keypair());
        assert!(
            accept_wrap_offer(&off_evil, id_a, id_b, &pk_bound, &chx, kp_a.secret).is_none(),
            "自报公钥与绑定值不符必须失败"
        );
    }

    #[test]
    fn wrap_rejects_foreign_channel() {
        let (_sk_a, pk_a) = ident();
        let (sk_b, pk_b) = ident();
        let (id_a, id_b) = ("gosslan-aaa", "gosslan-bbb");
        let chx_right = channel_hex(&channel_hash(&pk_a, &pk_b, id_a, id_b, 1));
        let chx_other = channel_hex(&channel_hash(&pk_a, &pk_b, id_a, id_b, 999));
        let kp_a = generate_eph_keypair();
        let kp_b = generate_eph_keypair();
        let off_b = build_wrap_offer(id_b, id_a, &chx_right, &sk_b, &kp_b);
        assert!(
            accept_wrap_offer(&off_b, id_a, id_b, &pk_b, &chx_other, kp_a.secret).is_none(),
            "中继把电路串到别的会话上必须失败"
        );
    }

    #[test]
    fn wrap_rejects_swapped_role() {
        let (_sk_a, pk_a) = ident();
        let (sk_b, pk_b) = ident();
        let (id_a, id_b) = ("gosslan-aaa", "gosslan-bbb");
        let chx = channel_hex(&channel_hash(&pk_a, &pk_b, id_a, id_b, 1));
        let kp_a = generate_eph_keypair();
        let kp_b = generate_eph_keypair();
        let mut off_b = build_wrap_offer(id_b, id_a, &chx, &sk_b, &kp_b);
        off_b.role = "A".into(); // 篡改方向（签名随之失效）
        assert!(accept_wrap_offer(&off_b, id_a, id_b, &pk_b, &chx, kp_a.secret).is_none());
    }

    #[test]
    fn records_roundtrip_in_both_directions() {
        let (sk_a, pk_a) = ident();
        let (sk_b, pk_b) = ident();
        let (id_a, id_b) = ("gosslan-aaa", "gosslan-bbb");
        let chx = channel_hex(&channel_hash(&pk_a, &pk_b, id_a, id_b, 1));
        let kp_a = generate_eph_keypair();
        let kp_b = generate_eph_keypair();
        let off_a = build_wrap_offer(id_a, id_b, &chx, &sk_a, &kp_a);
        let off_b = build_wrap_offer(id_b, id_a, &chx, &sk_b, &kp_b);
        let sa = accept_wrap_offer(&off_b, id_a, id_b, &pk_b, &chx, kp_a.secret).unwrap();
        let sb = accept_wrap_offer(&off_a, id_b, id_a, &pk_a, &chx, kp_b.secret).unwrap();

        let mut seal_a = RecordSeal::new(sa.out_key);
        let mut open_b = RecordOpen::new(sb.in_key);
        for i in 0..5 {
            let frame = format!("frame-{i}").into_bytes();
            let rec = seal_a.seal_frame(&frame).unwrap();
            assert_eq!(rec.len(), frame.len() + CTR_LEN + RECORD_OVERHEAD);
            assert_eq!(open_b.open_record(&rec).unwrap(), frame);
        }
        // 反向
        let mut seal_b = RecordSeal::new(sb.out_key);
        let mut open_a = RecordOpen::new(sa.in_key);
        let rec = seal_b.seal_frame(b"back").unwrap();
        assert_eq!(open_a.open_record(&rec).unwrap(), b"back");
        // 用错方向密钥解不开
        let mut wrong = RecordOpen::new(sa.out_key);
        let rec2 = seal_b.seal_frame(b"again").unwrap();
        assert!(wrong.open_record(&rec2).is_none());
    }

    #[test]
    fn replayed_record_is_rejected() {
        let key = [5u8; 32];
        let mut seal = RecordSeal::new(key);
        let mut open = RecordOpen::new(key);
        let rec = seal.seal_frame(b"frame-1").unwrap();
        assert!(open.open_record(&rec).is_some());
        assert!(open.open_record(&rec).is_none(), "重放必须失败");
    }

    #[test]
    fn gap_in_counter_is_rejected() {
        let key = [7u8; 32];
        let mut seal = RecordSeal::new(key);
        let mut open = RecordOpen::new(key);
        let _ = seal.seal_frame(b"one").unwrap();
        let two = seal.seal_frame(b"two").unwrap();
        assert!(open.open_record(&two).is_none(), "计数器缺口必须失败");
    }

    #[test]
    fn tampered_record_is_rejected() {
        let key = [9u8; 32];
        let mut seal = RecordSeal::new(key);
        let mut open = RecordOpen::new(key);
        let mut rec = seal.seal_frame(b"payload").unwrap();
        let n = rec.len() - 1;
        rec[n] ^= 0x01;
        assert!(open.open_record(&rec).is_none());
    }

    /// **核心等价性**：套上记录层之后，既有的 `write_bytes` / `read_bytes`
    /// 必须逐字节等价地跑通 —— 这是"上层协议（`Hello`、`writer_loop`、`reader_loop`）
    /// 一行都不用改"的根据。
    ///
    /// 三种 socket 缓冲都要跑：`8` 字节比一帧小两个数量级，专门用来踩
    /// `poll_write` 返回 `Pending` 的那条路 —— 背压下的"字节被吞两遍/漏一遍"
    /// 只在这里现形，宽缓冲的测试永远抓不到。
    #[tokio::test]
    async fn sealed_halves_carry_frames_byte_identically() {
        async fn roundtrip(socket_buf: usize, frames: usize) {
            let key = [11u8; 32];
            let (a, b) = tokio::io::duplex(socket_buf);
            // split 出来的是 (ReadHalf, WriteHalf)。写侧要 WriteHalf（第二个）、
            // 读侧要 ReadHalf（第一个）；**没用的那一半必须留在本作用域直到测试结束**，
            // 否则整个流被 drop，读侧会拿到人为的 EOF。
            let (_a_read, a_write) = tokio::io::split(a);
            let (b_read, _b_write) = tokio::io::split(b);
            let mut w = crate::transport::tcp::TcpSender::new(a_write);
            w.enable_seal(key);
            let mut r = crate::transport::tcp::TcpReceiver::new(b_read);
            r.enable_seal(key);

            let payloads: Vec<Vec<u8>> = (0..frames)
                .map(|i| {
                    format!("{{\"type\":\"heartbeat\",\"i\":{i}}}{}", "x".repeat(i % 97))
                        .into_bytes()
                })
                .collect();
            let send = payloads.clone();
            let tx = tokio::spawn(async move {
                for p in &send {
                    write_bytes(&mut w, p).await.expect("写入必须成功");
                }
            });
            let rx = tokio::spawn(async move {
                let mut got = Vec::new();
                for _ in 0..frames {
                    got.push(read_bytes(&mut r).await.expect("读取必须成功"));
                }
                got
            });
            tx.await.unwrap();
            assert_eq!(rx.await.unwrap(), payloads, "缓冲={socket_buf}");
        }
        roundtrip(64 * 1024, 200).await; // 宽缓冲：一次写完
        roundtrip(8, 50).await; // 比一帧小两个数量级：每条记录横跨多次 poll_*
        roundtrip(1, 20).await; // 最坏切分：每次只推进 1 字节
    }

    /// 对端密钥不同 ⇒ 一条都解不开（不是"解出错乱内容"，而是立刻报错断链）。
    #[tokio::test]
    async fn sealed_reader_rejects_other_key() {
        let (a, b) = tokio::io::duplex(4096);
        let (_a_read, a_write) = tokio::io::split(a);
        let (b_read, _b_write) = tokio::io::split(b);
        let mut w = crate::transport::tcp::TcpSender::new(a_write);
        w.enable_seal([1u8; 32]);
        let mut r = crate::transport::tcp::TcpReceiver::new(b_read);
        r.enable_seal([2u8; 32]);
        let tx = tokio::spawn(async move {
            write_bytes(&mut w, b"secret").await.unwrap();
        });
        let err = read_bytes(&mut r).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let _ = tx.await;
    }

    /// 半条记录后断开不得被当成"正常读完"。
    #[tokio::test]
    async fn truncated_record_is_an_error_not_a_clean_eof() {
        let key = [3u8; 32];
        let (mut a, b) = tokio::io::duplex(4096);
        let mut seal = RecordSeal::new(key);
        let rec = seal.seal_frame(b"\x00\x00\x01\x00partial").unwrap();
        let mut bytes = (rec.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(&rec);
        let half = bytes.len() / 2;
        a.write_all(&bytes[..half]).await.unwrap();
        a.flush().await.unwrap();
        drop(a); // 半条记录 + EOF
        let mut r = crate::transport::tcp::TcpReceiver::new(b);
        r.enable_seal(key);
        let err = read_bytes(&mut r).await.unwrap_err();
        assert!(
            err.kind() == std::io::ErrorKind::UnexpectedEof
                || err.kind() == std::io::ErrorKind::InvalidData,
            "残缺记录必须报错，实际 {:?}",
            err.kind()
        );
    }

    /// 记录层不得让帧"看起来变大到越过预认证闸门"：
    /// 内层长度前缀仍是唯一判据，外层只多 `RECORD_OVERHEAD + 计数器`。
    #[test]
    fn record_size_is_predictable() {
        let mut seal = RecordSeal::new([4u8; 32]);
        let frame = vec![9u8; 100];
        let rec = seal.seal_frame(&frame).unwrap();
        assert_eq!(rec.len(), frame.len() + CTR_LEN + RECORD_OVERHEAD);
    }
}
