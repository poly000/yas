use std::io::stdin;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use yas::capture::{capture_absolute, capture_absolute_image};
use yas::common::utils;
use yas::common::{PixelRect, RawImage};
use yas::expo::good::GOODFormat;
use yas::expo::mingyu_lab::MingyuLabFormat;
use yas::expo::mona_uranai::MonaFormat;
use yas::inference::inference::CRNNModel;
use yas::inference::pre_process::{
    crop, image_to_raw, normalize, pre_process, raw_to_img, to_gray,
};
use yas::info::info;
use yas::scanner::yas_scanner::{YasScanner, YasScannerConfig};

use clap::{App, Arg};
use env_logger::{Builder, Env, Target};
use image::imageops::grayscale;
use image::{ImageBuffer, Pixel};
use log::{info, warn, LevelFilter};
use os_info;

#[cfg(windows)]
fn detect_gi_window() -> (PixelRect, bool) {
    let rect: PixelRect;
    let is_cloud: bool;
    use winapi::shared::windef::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE;
    use winapi::um::wingdi::{GetDeviceCaps, HORZRES};
    use winapi::um::winuser::{
        GetDpiForSystem, GetSystemMetrics, SetForegroundWindow, SetProcessDPIAware,
        SetThreadDpiAwarenessContext, ShowWindow, SW_RESTORE, SW_SHOW,
    };
    // use winapi::um::shellscalingapi::{SetProcessDpiAwareness, PROCESS_PER_MONITOR_DPI_AWARE};

    crate::utils::set_dpi_awareness();

    let hwnd = match utils::find_window_local() {
        Ok(h) => {
            is_cloud = false;
            h
        }
        Err(_) => match utils::find_window_cloud() {
            Ok(h) => {
                is_cloud = true;
                h
            }
            Err(_) => utils::error_and_quit("未找到原神窗口，请确认原神已经开启"),
        },
    };

    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
    }
    // utils::sleep(1000);
    unsafe {
        SetForegroundWindow(hwnd);
    }
    utils::sleep(1000);

    rect = utils::get_client_rect(hwnd).unwrap();
}

#[cfg(all(target_os = "linux"))]
fn detect_gi_window() -> (PixelRect, bool) {
    let rect: PixelRect;
    let is_cloud: bool;
    let window_id = unsafe {
        String::from_utf8_unchecked(
            std::process::Command::new("sh")
                .arg("-c")
                .arg(r#" xwininfo|grep "Window id"|cut -d " " -f 4 "#)
                .output()
                .unwrap()
                .stdout,
        )
    };
    let window_id = window_id.trim_end_matches("\n");

    let position_size = unsafe {
        String::from_utf8_unchecked(
                std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&format!(r#" xwininfo -id {window_id}|cut -f 2 -d :|tr -cd "0-9\n"|grep -v "^$"|sed -n "1,2p;5,6p" "#))
                    .output()
                    .unwrap()
                    .stdout,
            )
    };

    let mut info = position_size.split("\n");

    let left = info.next().unwrap().parse().unwrap();
    let top = info.next().unwrap().parse().unwrap();
    let width = info.next().unwrap().parse().unwrap();
    let height = info.next().unwrap().parse().unwrap();

    rect = PixelRect {
        left,
        top,
        width,
        height,
    };
    is_cloud = false; // todo: detect cloud genshin by title

    (rect, is_cloud)
}

fn open_local(path: String) -> RawImage {
    let img = image::open(path).unwrap();
    let img = grayscale(&img);
    let raw_img = image_to_raw(img);

    raw_img
}

fn main() {
    Builder::new().filter_level(LevelFilter::Info).init();

    #[cfg(windows)]
    if !utils::is_admin() {
        utils::error_and_quit("请以管理员身份运行该程序")
    }

    if let Some(v) = utils::check_update() {
        warn!("检测到新版本，请手动更新：{}", v);
    }

    let matches = App::new("YAS - 原神圣遗物导出器")
        .version(utils::VERSION)
        .author("wormtql <584130248@qq.com>")
        .about("Genshin Impact Artifact Exporter")
        .arg(
            Arg::with_name("max-row")
                .long("max-row")
                .takes_value(true)
                .help("最大扫描行数"),
        )
        .arg(
            Arg::with_name("dump")
                .long("dump")
                .required(false)
                .takes_value(false)
                .help("输出模型预测结果、二值化图像和灰度图像，debug专用"),
        )
        .arg(
            Arg::with_name("capture-only")
                .long("capture-only")
                .required(false)
                .takes_value(false)
                .help("只保存截图，不进行扫描，debug专用"),
        )
        .arg(
            Arg::with_name("min-star")
                .long("min-star")
                .takes_value(true)
                .help("最小星级"),
        )
        .arg(
            Arg::with_name("min-level")
                .long("min-level")
                .takes_value(true)
                .help("最小等级"),
        )
        .arg(
            Arg::with_name("max-wait-switch-artifact")
                .long("max-wait-switch-artifact")
                .takes_value(true)
                .help("切换圣遗物最大等待时间(ms)"),
        )
        .arg(
            Arg::with_name("output-dir")
                .long("output-dir")
                .short("o")
                .takes_value(true)
                .help("输出目录")
                .default_value("."),
        )
        .arg(
            Arg::with_name("scroll-stop")
                .long("scroll-stop")
                .takes_value(true)
                .help("翻页时滚轮停顿时间（ms）（翻页不正确可以考虑加大该选项，默认为80）"),
        )
        .arg(
            Arg::with_name("number")
                .long("number")
                .takes_value(true)
                .help("指定圣遗物数量（在自动识别数量不准确时使用）"),
        )
        .arg(
            Arg::with_name("verbose")
                .long("verbose")
                .help("显示详细信息"),
        )
        .arg(
            Arg::with_name("offset-x")
                .long("offset-x")
                .takes_value(true)
                .help("人为指定横坐标偏移（截图有偏移时可用该选项校正）"),
        )
        .arg(
            Arg::with_name("offset-y")
                .long("offset-y")
                .takes_value(true)
                .help("人为指定纵坐标偏移（截图有偏移时可用该选项校正）"),
        )
        .arg(
            Arg::with_name("output-format")
                .long("output-format")
                .short("f")
                .takes_value(true)
                .help("输出格式")
                .possible_values(&["mona", "mingyulab", "good", "all"])
                .default_value("mona"),
        )
        .arg(
            Arg::with_name("cloud-wait-switch-artifact")
                .long("cloud-wait-switch-artifact")
                .takes_value(true)
                .help("指定云·原神切换圣遗物等待时间(ms)"),
        )
        .get_matches();
    let config = YasScannerConfig::from_match(&matches);

    let rect: PixelRect;
    let is_cloud: bool;
    (rect, is_cloud) = detect_gi_window();

    // rect.scale(1.25);
    info!(
        "left = {}, top = {}, width = {}, height = {}",
        rect.left, rect.top, rect.width, rect.height
    );

    let mut info: info::ScanInfo;
    if rect.height * 43 == rect.width * 18 {
        info =
            info::ScanInfo::from_43_18(rect.width as u32, rect.height as u32, rect.left, rect.top);
    } else if rect.height * 16 == rect.width * 9 {
        info =
            info::ScanInfo::from_16_9(rect.width as u32, rect.height as u32, rect.left, rect.top);
    } else if rect.height * 8 == rect.width * 5 {
        info = info::ScanInfo::from_8_5(rect.width as u32, rect.height as u32, rect.left, rect.top);
    } else if rect.height * 4 == rect.width * 3 {
        info = info::ScanInfo::from_4_3(rect.width as u32, rect.height as u32, rect.left, rect.top);
    } else if rect.height * 7 == rect.width * 3 {
        info = info::ScanInfo::from_7_3(rect.width as u32, rect.height as u32, rect.left, rect.top);
    } else {
        utils::error_and_quit("不支持的分辨率");
    }

    let offset_x = matches
        .value_of("offset-x")
        .unwrap_or("0")
        .parse::<i32>()
        .unwrap();
    let offset_y = matches
        .value_of("offset-y")
        .unwrap_or("0")
        .parse::<i32>()
        .unwrap();
    info.left += offset_x;
    info.top += offset_y;

    let mut scanner = YasScanner::new(info.clone(), config, is_cloud);

    let now = SystemTime::now();
    let results = scanner.start();
    let t = now.elapsed().unwrap().as_secs_f64();
    info!("time: {}s", t);

    let output_dir = Path::new(matches.value_of("output-dir").unwrap());

    if let Some(output_format) = matches.value_of("output-format") {
        let filename = output_dir.join(format!("{output_format}.json"));
        let filename = filename.to_str().unwrap();

        match output_format {
            "mona" => MonaFormat::new(&results).save(filename),
            "mingyulab" => MingyuLabFormat::new(&results).save(filename),
            "good" => GOODFormat::new(&results).save(filename),
            "all" => {
                MonaFormat::new(&results).save(filename);
                MingyuLabFormat::new(&results).save(filename);
                GOODFormat::new(&results).save(filename);
            }
            _ => unreachable!(""),
        }
    }
    // let info = info;
    // let img = info.art_count_position.capture_relative(&info).unwrap();

    // let mut inference = CRNNModel::new(String::from("model_training.onnx"), String::from("index_2_word.json"));
    // let s = inference.inference_string(&img);
    // println!("{}", s);
    info!("识别结束，请按Enter退出");
    let mut s = String::new();
    stdin().read_line(&mut s);
}
