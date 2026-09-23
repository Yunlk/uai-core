//! 登录态。
//!
//! ## 两套令牌分开存，永远别混
//!
//! | 字段 | 用途 | 打到哪个域名 |
//! | --- | --- | --- |
//! | [`Session::portal_token`] | 门户 SSO JWT，走 `Authorization` | `uai.unipus.cn` |
//! | [`Session::open_id`] | 账户标识，用于**现铸** annotator token | — |
//!
//! annotator token **不存**，每次用时现铸：它有效期一年，而 `open_id` 才是
//! 真正的凭据；存下来只会让「令牌过期了但没人知道」这类问题更难查。
//! [`Session::annotator_token`] 内部调 [`crate::annotator::mint`]。

use serde::{Deserialize, Serialize};

use crate::annotator;
use crate::error::Result;

/// 登录成功后的最小状态。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// 门户 SSO JWT。空串表示未登录。
    pub portal_token: String,
    /// ucontent 的 `open_id`，来自门户用户信息的 **`ssoId`**。
    ///
    /// ⚠️ **不是 `appUserId`。** 实测只有 `ssoId` 能让进度接口返回 200；
    /// 传 `appUserId` 会 401。
    pub open_id: String,
    /// 展示名，仅用于打印。
    pub display_name: String,
    /// 学校号，来自用户信息的 `school`。**打 `/api/aca/*` 必须有。**
    #[serde(default)]
    pub school: String,
    /// 门户自己的用户 ID，来自 `appUserId`。
    ///
    /// 只用于**标识**（例如将来查他人成绩），**不要**拿它当 `open_id`。
    #[serde(default)]
    pub app_user_id: String,
}

impl Session {
    /// 是否已登录（有门户 JWT）。
    pub fn is_authenticated(&self) -> bool {
        !self.portal_token.trim().is_empty()
    }

    /// 是否能访问 ucontent 的受保护接口（有 `open_id`）。
    ///
    /// 目录/内容接口不需要它，进度/答案/提交需要。
    pub fn can_touch_protected(&self) -> bool {
        !self.open_id.trim().is_empty()
    }

    /// 现铸一个 annotator token。
    pub fn annotator_token(&self) -> Result<String> {
        annotator::mint(&self.open_id)
    }

    /// 是否能打 `/api/aca/*`（考核方案/综合成绩）。
    ///
    /// 这类接口要 `u-school` + `u-openid`，缺了会被服务端当 `6011 feature denied`。
    pub fn can_touch_assessment(&self) -> bool {
        !self.school.trim().is_empty() && !self.open_id.trim().is_empty()
    }

    /// 构造考核接口要的宿主身份。
    pub fn host_identity(&self) -> crate::transport::HostIdentity {
        crate::transport::HostIdentity::new(self.school.clone(), self.open_id.clone())
    }

    /// 打日志时的安全摘要——**绝不打印令牌本身**。
    pub fn describe(&self) -> String {
        format!(
            "{}（门户令牌 {}，open_id {}）",
            if self.display_name.trim().is_empty() {
                "已登录用户"
            } else {
                &self.display_name
            },
            if self.is_authenticated() {
                "已获取"
            } else {
                "缺失"
            },
            if self.can_touch_protected() {
                "已获取"
            } else {
                "缺失"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_session_is_not_authenticated() {
        let session = Session::default();
        assert!(!session.is_authenticated());
        assert!(!session.can_touch_protected());
    }

    /// 有门户令牌但没有 open_id 时，不能打受保护接口——
    /// 这是旧工程踩过的坑（用门户 JWT 打进度接口拿到 401 的 HTML）。
    #[test]
    fn portal_token_alone_cannot_touch_protected() {
        let session = Session {
            portal_token: "jwt".to_owned(),
            ..Default::default()
        };
        assert!(session.is_authenticated());
        assert!(!session.can_touch_protected());
        assert!(session.annotator_token().is_err());
    }

    /// `describe` 不能泄露令牌内容。
    #[test]
    fn describe_never_leaks_token() {
        let session = Session {
            portal_token: "super-secret-jwt".to_owned(),
            open_id: "00000000000000000000000000000000".to_owned(),
            display_name: "测试".to_owned(),
            school: "1000".to_owned(),
            app_user_id: "1581876226543616901".to_owned(),
        };
        let text = session.describe();
        assert!(!text.contains("super-secret-jwt"), "{text}");
        assert!(!text.contains("00000000000000000000000000000000"), "{text}");
        assert!(text.contains("已获取"), "{text}");
    }

    /// ★ `describe` 也不能泄露 `appUserId`。
    #[test]
    fn describe_never_leaks_app_user_id() {
        let session = Session {
            portal_token: "jwt".to_owned(),
            open_id: "open".to_owned(),
            app_user_id: "1581876226543616901".to_owned(),
            school: "1000".to_owned(),
            ..Default::default()
        };
        assert!(!session.describe().contains("1581876226543616901"));
    }

    /// ★ 打考核接口要 `school` + `open_id`，缺一不可。
    #[test]
    fn assessment_requires_school_and_open_id() {
        let full = Session {
            open_id: "open".to_owned(),
            school: "1000".to_owned(),
            ..Default::default()
        };
        assert!(full.can_touch_assessment());

        let no_school = Session {
            open_id: "open".to_owned(),
            ..Default::default()
        };
        assert!(!no_school.can_touch_assessment());

        let no_open = Session {
            school: "1000".to_owned(),
            ..Default::default()
        };
        assert!(!no_open.can_touch_assessment());
    }

    /// `host_identity` 要把 school / open_id 装进宿主头，并用实测默认值。
    #[test]
    fn host_identity_carries_school_and_open_id() {
        let session = Session {
            open_id: "00000000000000000000000000000000".to_owned(),
            school: "1000".to_owned(),
            ..Default::default()
        };
        let host = session.host_identity();
        assert_eq!(host.school, "1000");
        assert_eq!(host.open_id, "00000000000000000000000000000000");
        assert_eq!(host.app_id, "116");
        assert_eq!(host.platform, "2");
        assert!(host.is_complete());
    }
}
