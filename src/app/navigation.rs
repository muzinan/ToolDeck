//! 导航侧栏使用的纯导航数据处理。

use crate::tools::ToolDescriptor;

/// 按工具注册顺序返回有效收藏，重复和失效 ID 会被忽略。
pub(super) fn favorite_descriptors(
    descriptors: &[ToolDescriptor],
    favorite_tools: &[String],
) -> Vec<ToolDescriptor> {
    descriptors
        .iter()
        .filter(|descriptor| {
            favorite_tools
                .iter()
                .any(|favorite| favorite == descriptor.id)
        })
        .cloned()
        .collect()
}

/// 按当前注册顺序保留收藏项，清除旧版本遗留的无效或重复工具标识。
pub(super) fn normalize_favorite_tools(
    descriptors: &[ToolDescriptor],
    favorite_tools: &mut Vec<String>,
) {
    let selected: std::collections::HashSet<_> =
        favorite_tools.iter().map(String::as_str).collect();
    *favorite_tools = descriptors
        .iter()
        .filter(|descriptor| selected.contains(descriptor.id))
        .map(|descriptor| descriptor.id.to_owned())
        .collect();
}
