//! 全部服务端端点与域名常量。
//!
//! ## 为什么单独一个文件
//!
//! 旧工程里同一个路径在多个文件里各写一遍（`format!("…/progress/v2")` 出现在
//! `content.rs` 和 `submit.rs`），改一处漏一处。这里所有路径**只在这里出现**，
//! 各端点模块只做参数拼装。
//!
//! ## 三个应用，别搞混
//!
//! | 应用 | 域名 | 鉴权 | 成功码 |
//! | --- | --- | --- | --- |
//! | portal 门户 | `uai.unipus.cn` | 门户 SSO JWT | `code: 1` |
//! | SSO 统一登录 | `sso.unipus.cn` | — | `code: "0"` |
//! | ucontent 内容 | `ucontent.unipus.cn` | 自铸 annotator token | `code: 0` |
//!
//! ⚠️ 成功码**不统一**：门户是 `1`，ucontent 是 `0`，SSO 是字符串 `"0"`。
//! 混用会让「明明成功了却判失败」。

/// 门户与统一登录。
pub const PORTAL_BASE: &str = "https://uai.unipus.cn";
pub const SSO_BASE: &str = "https://sso.unipus.cn";

/// 内容平台。`/api/tla/*` 之类的门户接口**不在**这个域名下。
pub const UCONTENT_BASE: &str = "https://ucontent.unipus.cn";

/// 门户 SSO 服务标识。
pub const PORTAL_SERVICE: &str = "https://uai.unipus.cn/portal";

/// UMCS 二维码登录的 Socket.IO 地址。
pub const UMCS_SOCKET_BASE: &str = "wss://umcs.unipus.cn";

/// 时长加速的 Socket.IO（engine.io v3）地址。
pub const DURATION_SOCKET_BASE: &str = "wss://ucontent.unipus.cn/unipusiopoint/";

/// ucontent 受保护接口要求的鉴权头名。
///
/// ⚠️ **不要改回 `Authorization`。** 2026-09 实测：`Authorization` 打进度/答案会 401，
/// 只有这个名字能通。目录与内容接口不校验鉴权，发错头也照样 `code:0`——
/// 所以只测那两个接口是发现不了这个错误的。
pub const AUTH_HEADER: &str = "x-annotator-auth-token";

/// 内容/答案/进度接口固定使用的模式参数。
pub const DEFAULT_MODE: &str = "default";

// ─────────────────────────── SSO 登录 ───────────────────────────

/// 密码登录。body `{service, username, password}`，用户名密码需 AES-128-CBC 加密。
pub const SSO_PASSWORD_LOGIN: &str = "/sso/0.1/sso/cip/login";

/// 申请二维码。body `{deviceId, service}`。
pub const SSO_QR_CODE: &str = "/sso/7.0/sso/code";

/// 用 serviceTicket 换 JWT。query `?ticket=&service=`。
pub const SSO_TICKET_VALIDATE: &str = "/sso/serviceTicket/validate";

/// 二维码登录补充 cookie。
pub const SSO_ADD_COOKIE: &str = "/sso/7.0/sso/add/cookie";

// ─────────────────────────── 门户 ───────────────────────────

/// 当前用户信息（含 `ssoId`）。
pub const PORTAL_USER_INFO: &str = "/api/account/user/info";

/// 门户书架。**不返回进度**。
pub const PORTAL_BOOKSHELF: &str = "/api/cmgt/course/my/bookshelf";

/// 按教学班给出的**真实进度**（`finishPointNum`/`totalPointNum`）。
pub const PORTAL_COURSE_LIST: &str = "/api/cmgt/course/getCourseListByStudent";

/// 教材激活标记。body `{resourceId}`。只读，不激活。
pub const PORTAL_ACTIVE_FLAG: &str = "/api/product/course/courseActiveFlag";

/// 单个班级教材的完整信息。
///
/// `GET /api/cmgt/course/getCourseResourceInfoById/{courseResourceId}`
///
/// 返回 `classId` / `courseId` / `strategyId` / `courseInstanceId`，是**考核接口
/// 唯一可靠的入参来源**——考核接口要的 `courseInstanceId` 是数字 `courseId`，
/// 不是 `course-v2:` 那串实例 ID（见 [`crate::api::assessment`]）。
pub const PORTAL_COURSE_RESOURCE_INFO: &str = "/api/cmgt/course/getCourseResourceInfoById";

/// 考核方案：权重、时长标准、记分周期。`POST`，body `{courseInstanceId, classId}`。
pub const PORTAL_ASSESS_PLAN: &str = "/api/aca/plan/detail";

/// 综合成绩：当前成绩 + 各考核项得分。`POST`，body `{courseInstanceId, classId}`。
pub const PORTAL_ASSESS_SCORE: &str = "/api/aca/achievement/queryUserScore";

// ────────────────── 门户「微应用宿主」请求头 ──────────────────
//
// ⚠️ **`/api/aca/*` 是受保护的。** 只带 `Authorization` 会拿到
// `{"code":6011,"msg":"feature denied"}`——不是权限不足，是**认不出是哪个应用**。
//
// 门户前端是 qiankun 微应用架构：宿主 `cloud-pc`（`index-*.js`）在挂载微应用时把
// `getExtraHeaders()` 注入 `window[微应用名]`，微应用再把它并进 axios 默认头。
// 宿主的实现（`cloud-pc` 模块 `49405`）就是下面这四个头。
//
// 实测：缺 `u-app-id` → `6011`；带上 → 正常返回。`u-school` 填错也会 `6011`。

/// 宿主应用 ID。宿主模块 `94863` 里的常量 `$m`，实测 **116**。
pub const HOST_APP_ID: &str = "116";

/// 宿主平台号。宿主枚举 `OD { MOBILE=1, PC=2, PAD=3 }`，网页端是 **2**。
pub const HOST_PLATFORM_PC: &str = "2";

pub const HOST_APP_ID_HEADER: &str = "u-app-id";
pub const HOST_PLATFORM_HEADER: &str = "u-platform";
pub const HOST_SCHOOL_HEADER: &str = "u-school";
pub const HOST_OPENID_HEADER: &str = "u-openid";

// ─────────────────────────── ucontent ───────────────────────────

/// 课程目录（单元 → 分组）。
///
/// `courseId` **不能 URL 编码**，否则服务端返回空壳 `code:2`。
/// `course-v1:`（资源 ID）与 `course-v2:`（实例 ID）都接受。
pub const COURSE_CATALOG: &str = "/course/api/course";

/// 分组题目内容（AES 加密）。
///
/// 两种 ID 返回**不同 schema**：
/// - `course-v1:` → 对象，key 形如 `questions:questions`（界面预览用）
/// - `course-v2:` → 数组，元素 `{"id":…,"content":"<JSON 字符串>"}`（**提交必须用这个**）
pub const GROUP_CONTENT: &str = "/course/api/v3/content";

/// 标准答案与解析（AES 加密）。
pub const GROUP_ANSWER: &str = "/course/api/v3/answer";

/// **提交作答**。唯一会写服务端的接口。
pub const SUBMIT: &str = "/course/api/v3/newExploration/submit";

/// 分组状态（是否做过/通过）。**单条任务状态的真源。**
pub const GROUP_STATE: &str = "/api/mobile/user_module";

/// `progress/v2` 后缀。拼在 `/…/{groupId}` 之后。
pub const GROUP_STATE_SUFFIX: &str = "/progress/v2";

/// 整课单元级进度。
pub const COURSE_PROGRESS: &str = "/course/api/v2/course_progress";

/// 目录级批量进度。
///
/// ⚠️ **仅供排查，不可用作完成度**：对从未做过的教材也回 `state.pass:1`。
pub const COURSE_PROGRESS_TASKS: &str = "/course/api/v2/course_progress";

/// 口语录音评分上报。**无服务端评测**，分数由客户端自报。
pub const VOICE_EVALUATION: &str = "/course/api/v3/voiceEvaluation";

/// 录音文件上传。接上它才能拿真实录音分数。
pub const MEDIA_UPLOAD: &str = "/media/user_resource/fetch";

// ─────────────────────────── 拼装辅助 ───────────────────────────

/// 拼出目录 URL。
pub fn catalog_url(course_id: &str) -> String {
    format!("{UCONTENT_BASE}{COURSE_CATALOG}/{course_id}/{DEFAULT_MODE}")
}

/// 拼出分组内容 URL。`course_id` 原样拼入，**不做 URL 编码**。
pub fn content_url(course_id: &str, group_id: &str) -> String {
    format!("{UCONTENT_BASE}{GROUP_CONTENT}/{course_id}/{group_id}/{DEFAULT_MODE}")
}

/// 拼出标准答案 URL。
pub fn answer_url(course_id: &str, group_id: &str) -> String {
    format!("{UCONTENT_BASE}{GROUP_ANSWER}/{course_id}/{group_id}/{DEFAULT_MODE}")
}

/// 拼出分组状态 URL。
pub fn group_state_url(course_id: &str, group_id: &str) -> String {
    format!("{UCONTENT_BASE}{GROUP_STATE}/{course_id}/{group_id}{GROUP_STATE_SUFFIX}")
}

/// 拼出作答快照 URL。
///
/// ⚠️ `last_submit` **必填**。少了它服务端回
/// `{"code":1,"data":null,"msg":"data not exist"}`。
pub fn snapshot_url(course_id: &str, group_id: &str, last_submit: &str) -> String {
    format!("{UCONTENT_BASE}{GROUP_STATE}/{course_id}/{group_id}-{last_submit}")
}

/// 拼出整课单元级进度 URL。
pub fn course_progress_url(course_id: &str, open_id: &str) -> String {
    format!("{UCONTENT_BASE}{COURSE_PROGRESS}/{course_id}/{open_id}/{DEFAULT_MODE}")
}

/// 拼出目录级批量进度 URL。`tasks` 必须是**逗号分隔**的分组 ID。
pub fn course_progress_tasks_url(course_id: &str, open_id: &str, tasks: &str) -> String {
    format!(
        "{UCONTENT_BASE}{COURSE_PROGRESS_TASKS}/{course_id}/tasks/{open_id}/{DEFAULT_MODE}?tasks={tasks}"
    )
}

/// 拼出提交 URL。
pub fn submit_url() -> String {
    format!("{UCONTENT_BASE}{SUBMIT}")
}

/// 拼出门户接口 URL。
pub fn portal_url(path: &str) -> String {
    format!("{PORTAL_BASE}{path}")
}

/// 拼出「班级教材详情」URL。
pub fn course_resource_info_url(course_resource_id: &str) -> String {
    format!("{PORTAL_BASE}{PORTAL_COURSE_RESOURCE_INFO}/{course_resource_id}")
}

/// 拼出考核类接口 URL。`path` 传 [`PORTAL_ASSESS_PLAN`] 或 [`PORTAL_ASSESS_SCORE`]。
pub fn assess_url(path: &str) -> String {
    format!("{PORTAL_BASE}{path}")
}

/// 拼出 SSO 接口 URL。
pub fn sso_url(path: &str) -> String {
    format!("{SSO_BASE}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内容接口的 `courseId` **不能**被 URL 编码：`:` 必须原样出现在路径里。
    #[test]
    fn catalog_url_keeps_colon_unencoded() {
        let url = catalog_url("course-v2:abc+def+2024");
        assert!(url.contains("/course-v2:abc+def+2024/"), "{url}");
        assert!(!url.contains("%3A"), "冒号被编码了：{url}");
    }

    /// 快照 URL 必须把 `lastSubmit` 用 `-` 拼在 groupId 后面。
    #[test]
    fn snapshot_url_appends_last_submit_with_dash() {
        let url = snapshot_url("course-v2:x", "u5g341", "1780120650");
        assert!(url.ends_with("/u5g341-1780120650"), "{url}");
    }

    /// 批量进度必须用逗号分隔，空格会让服务端静默返回 `leafs:{}`。
    #[test]
    fn tasks_url_joins_with_commas() {
        let url = course_progress_tasks_url("course-v2:x", "open", "u1g1,u1g2");
        assert!(url.ends_with("?tasks=u1g1,u1g2"), "{url}");
    }

    /// 三个域名不能串。
    #[test]
    fn bases_are_distinct_hosts() {
        assert!(PORTAL_BASE.contains("uai.unipus.cn"));
        assert!(SSO_BASE.contains("sso.unipus.cn"));
        assert!(UCONTENT_BASE.contains("ucontent.unipus.cn"));
        assert_ne!(PORTAL_BASE, UCONTENT_BASE);
    }
}
