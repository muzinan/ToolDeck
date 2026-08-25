//! 应用外壳的后台事件接收规则。

use std::collections::HashMap;

use crate::core::worker::RequestId;

/// 判断后台事件是否属于当前工具的最新请求。
pub(super) fn accept_task_event(
    latest_requests: &mut HashMap<&'static str, RequestId>,
    tool_id: &'static str,
    request_id: RequestId,
    is_finished: bool,
) -> bool {
    if latest_requests.get(tool_id).copied() != Some(request_id) {
        return false;
    }
    if is_finished {
        latest_requests.remove(tool_id);
    }
    true
}
