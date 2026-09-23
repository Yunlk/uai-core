//! 两处 AES 用法的唯一实现处。
//!
//! ## 为什么两种模式完全不同（别统一）
//!
//! | 用途 | 模式 | 密钥 | 填充 |
//! | --- | --- | --- | --- |
//! | 解密服务端返回的内容/答案 | **AES-128-ECB** | `"1a2b3c4d" + 响应里的 k` | **ZeroPadding**（手工去尾 `\0`） |
//! | 加密登录的用户名/密码 | **AES-128-CBC** | 硬编码 16 字节 | **PKCS7** |
//!
//! 把两者「统一」会立刻全盘失败：ECB 没有 IV，CBC 有；ZeroPadding 与 PKCS7
//! 处理尾部的方式相反。

use aes::{
    Aes128,
    cipher::{Block, BlockCipherDecrypt, BlockModeEncrypt, KeyInit, KeyIvInit},
};
use cbc::{Encryptor, cipher::block_padding::Pkcs7};

use crate::error::{Error, Result};

/// 内容解密的密钥前缀。真实密钥 = 这个 + 响应里的 `k` 字段（形如 `20260921`）。
const CONTENT_KEY_PREFIX: &str = "1a2b3c4d";

/// 密文前缀。不带这个前缀的响应按**明文**处理。
const CONTENT_BLOB_PREFIX: &str = "unipus.";

/// 登录字段加密的硬编码密钥（复刻官网 SDK）。
const LOGIN_KEY: [u8; 16] = [
    0x8A, 0xD7, 0x0B, 0x64, 0x1C, 0x02, 0x4C, 0x7A, 0xDA, 0x2E, 0xCD, 0x08, 0x2E, 0xC0, 0x33, 0x4F,
];

/// 登录字段加密的 IV。固定值，随 SDK 下发。
const LOGIN_IV: [u8; 16] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
];

/// 解密 ucontent 的 `content` / `answer` 字段。
///
/// 密文形如 `unipus.<hex>`，密钥是 `"1a2b3c4d" + k`（`k` 来自同一个响应）。
/// 不带 `unipus.` 前缀时**原样返回**——服务端对部分内容不做加密。
pub fn decrypt_blob(blob: &str, k: &str) -> Result<String> {
    let Some(hex_text) = blob.strip_prefix(CONTENT_BLOB_PREFIX) else {
        // 未加密时按原文返回，服务端可能对部分内容不做加密。
        return Ok(blob.to_owned());
    };

    let key = format!("{CONTENT_KEY_PREFIX}{k}");
    if key.len() != 16 {
        return Err(Error::decrypt(format!(
            "解密密钥长度不正确：k 字段为“{k}”，密钥应为 16 字节"
        )));
    }

    let data = hex::decode(hex_text)
        .map_err(|error| Error::decrypt(format!("解密内容不是合法十六进制：{error}")))?;
    if data.is_empty() || data.len() % 16 != 0 {
        return Err(Error::decrypt(format!(
            "密文长度 {} 不是 16 的整数倍",
            data.len()
        )));
    }

    let cipher = Aes128::new_from_slice(key.as_bytes())
        .map_err(|_| Error::decrypt("初始化 AES-128 失败"))?;
    let mut plain = Vec::with_capacity(data.len());
    for chunk in data.chunks(16) {
        let mut block = Block::<Aes128>::try_from(chunk)
            .map_err(|_| Error::decrypt("密文分组长度不是 16 字节"))?;
        cipher.decrypt_block(&mut block);
        plain.extend_from_slice(&block);
    }

    // ZeroPadding：手工去掉尾部 \0。
    while plain.last() == Some(&0) {
        plain.pop();
    }

    String::from_utf8(plain)
        .map_err(|error| Error::decrypt(format!("解密结果不是合法 UTF-8：{error}")))
}

/// 加密登录的用户名或密码，输出**大写十六进制**。
///
/// AES-128-CBC + PKCS7，输出 `hex::encode_upper`。大小写写错服务端会当密码错误。
pub fn encrypt_login_field(value: &str) -> Result<String> {
    type Aes128Cbc = Encryptor<Aes128>;
    let cipher = Aes128Cbc::new((&LOGIN_KEY).into(), (&LOGIN_IV).into());
    let ciphertext = cipher.encrypt_padded_vec::<Pkcs7>(value.as_bytes());
    Ok(hex::encode_upper(ciphertext))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不带 `unipus.` 前缀的响应按明文返回，不能报错。
    #[test]
    fn plain_text_passes_through() {
        let text = r#"{"code":0,"content":"hello"}"#;
        assert_eq!(decrypt_blob(text, "20260921").unwrap(), text);
    }

    /// `k` 长度不对必须报错，而不是解出乱码。
    #[test]
    fn wrong_key_length_is_rejected() {
        let error = decrypt_blob("unipus.00112233445566778899aabbccddeeff", "短").unwrap_err();
        assert!(error.message().contains("16 字节"), "{error}");
    }

    /// hex 非法必须报「不是合法十六进制」，而不是 panic。
    #[test]
    fn invalid_hex_is_rejected() {
        let error = decrypt_blob("unipus.zzzz", "20260921").unwrap_err();
        assert!(error.message().contains("十六进制"), "{error}");
    }

    /// 密文长度不是 16 的整数倍必须报错。
    #[test]
    fn unaligned_ciphertext_is_rejected() {
        let error = decrypt_blob("unipus.0011", "20260921").unwrap_err();
        assert!(error.message().contains("整数倍"), "{error}");
    }

    /// 登录字段加密输出必须是大写十六进制，且长度随 PKCS7 补齐到整块。
    #[test]
    fn login_field_is_uppercase_hex() {
        let encrypted = encrypt_login_field("798116aA").unwrap();
        assert!(
            encrypted
                .chars()
                .all(|c| c.is_ascii_digit() || ('A'..='G').contains(&c)),
            "应为大写十六进制：{encrypted}"
        );
        assert_eq!(encrypted.len() % 32, 0, "长度应是 16 字节块的倍数");
    }

    /// 同一输入必须稳定产出同一密文（IV 固定，不是随机 IV）。
    #[test]
    fn login_field_is_deterministic() {
        let a = encrypt_login_field("15702491291").unwrap();
        let b = encrypt_login_field("15702491291").unwrap();
        assert_eq!(a, b);
    }
}
