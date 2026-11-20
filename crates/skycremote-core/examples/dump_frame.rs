// 导出一帧 H.264出来，给界面那头的解码器做验证。
//
// 用法： cargo run -p skycremote-core --example dump_frame
// 写两个文件：
//   /tmp/skyc-frame.h264   Annex-B，也就是编码器直接吐出来的样子
//   /tmp/skyc-frame.json   帧的宽高、是不是关键帧、还有 base64
//
// 这不是产品的一部分，是给「界面能不能解开这帧」这件事准备材料。

use skycremote_core::capture::Frame;
use skycremote_core::encode::{Encoder, EncoderConfig};
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let width = 320u32;
    let height = 240u32;

    // 一张能看出内容的图：渐变加一个方块。
    let mut image = image::RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let red = (x * 255 / width) as u8;
            let green = (y * 255 / height) as u8;
            let blue = if x > width / 3 && x < width * 2 / 3 && y > height / 3 && y < height * 2 / 3
            {
                255
            } else {
                40
            };
            image.put_pixel(x, y, image::Rgba([red, green, blue, 255]));
        }
    }

    let frame = Frame {
        image,
        captured_at: Instant::now(),
    };

    let mut encoder = Encoder::new(EncoderConfig::for_screen(width, height))?;
    let encoded = encoder
        .encode(&frame)?
        .ok_or_else(|| anyhow::anyhow!("the encoder produced nothing"))?;

    std::fs::write("/tmp/skyc-frame.h264", &encoded.data)?;
    let text = serde_json::json!({
        "width": encoded.width,
        "height": encoded.height,
        "keyframe": encoded.keyframe,
        "bytes": encoded.data.len(),
        "base64": base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &encoded.data,
        ),
    });
    std::fs::write("/tmp/skyc-frame.json", serde_json::to_string(&text)?)?;

    println!("wrote /tmp/skyc-frame.h264 ({} bytes)", encoded.data.len());
    println!("keyframe: {}", encoded.keyframe);
    Ok(())
}
