//! 将 Windows 应用清单嵌入最终可执行文件，不在运行时依赖外部资源文件。

fn main() {
    embed_resource::compile("Toolbox.rc", embed_resource::NONE);
}
