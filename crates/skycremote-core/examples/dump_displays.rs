// 打印各个来源对显示器位置的看法，用来搞清楚坐标该怎么算。
//
// 跑法： cargo run -p skycremote-core --example dump_displays

fn main() {
    println!("== xcap ==");
    match xcap::Monitor::all() {
        Ok(monitors) => {
            for (index, monitor) in monitors.iter().enumerate() {
                println!(
                    "  {index}: {}x{} scale={:?}",
                    monitor.width().unwrap_or(0),
                    monitor.height().unwrap_or(0),
                    monitor.scale_factor()
                );
            }
        }
        Err(error) => println!("  读不到：{error}"),
    }

    println!("== display-info ==");
    for (index, info) in display_info::DisplayInfo::all().unwrap_or_default().iter().enumerate() {
        println!(
            "  {index}: id={} {}x{} at ({}, {}) scale={} primary={} name={:?}",
            info.id, info.width, info.height, info.x, info.y, info.scale_factor, info.is_primary, info.name
        );
    }
}
