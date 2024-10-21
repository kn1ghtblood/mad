#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use once_cell::sync::Lazy;
use regex::Regex;
use std::{
    ffi::CString,
    fs::{self, File},
    io::{self, Write},
    os::{raw::c_char, windows::process::CommandExt},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tokio::sync::{broadcast, mpsc};
use ureq;

#[link(name = "framecat", kind = "dylib")] // Link to the shared library (DLL)
extern "C" {
    fn fconcat(input_list_filename: *const c_char, output_filename: *const c_char) -> i32;
}

// const SAVE_PATH: &str = "temp";
const SAVE_PATH: &str = "downloads";
const VIDEO_M3U8_PREFIX: &str = "https://surrit.com/";
const VIDEO_PLAYLIST_SUFFIX: &str = "/playlist.m3u8";

static COUNTER: Lazy<Arc<Mutex<i32>>> = Lazy::new(|| Arc::new(Mutex::new(0)));

static GLOBAL_LOG: Lazy<Mutex<Vec<String>>> = Lazy::new(|| Mutex::new(Vec::new()));

pub fn log_message(message: String) {
    if let Ok(mut log) = GLOBAL_LOG.lock() {
        log.clear();
        log.push(message);
    }
}
struct VideoDownloader {
    input: String,
    progress: f32,
    is_downloading: bool,
    download_handle: Option<tokio::task::JoinHandle<()>>,
    progress_receiver: Option<mpsc::Receiver<f32>>,
    cancel_sender: Option<broadcast::Sender<()>>,
}

impl Default for VideoDownloader {
    fn default() -> Self {
        Self {
            input: String::new(),
            progress: 0.0,
            is_downloading: false,
            download_handle: None,
            progress_receiver: None,
            cancel_sender: None,
        }
    }
}

impl eframe::App for VideoDownloader {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("MADownloader");
                ui.add_space(20.0);

                // ui.horizontal(|ui| {
                ui.label("Enter the magic words:");
                // ui.text_edit_singleline(&mut self.input);
                ui.add(
                    egui::TextEdit::singleline(&mut self.input)
                        .hint_text("You know what to put here"),
                );
                // });

                ui.add_space(20.0);
                if !self.is_downloading {
                    // if ui.button("Download").clicked() {
                    if ui
                        .add_sized([120.0, 25.0], egui::Button::new("Download"))
                        .clicked()
                        && !self.input.is_empty()
                    {
                        self.start_download();
                    }
                } else {
                    // if ui.button("Cancel").clicked() {
                    if ui
                        .add_sized([120.0, 25.0], egui::Button::new("Cancel"))
                        .clicked()
                    {
                        self.cancel_download();
                        self.progress = 0.0;
                    }
                }
                ui.add_space(10.0);
                if ui
                    .add_sized([120.0, 25.0], egui::Button::new("Open Folder"))
                    .clicked()
                {
                    self.open_downloads_folder();
                }
                ui.add_space(20.0);
                if self.is_downloading {
                    ui.add(
                        egui::ProgressBar::new(self.progress)
                            .show_percentage()
                            .animate(true),
                    );
                }

                if let Some(receiver) = &mut self.progress_receiver {
                    if let Ok(progress) = receiver.try_recv() {
                        self.progress = progress;
                        if progress >= 1.0 {
                            self.is_downloading = false;
                        }
                    }
                }
                ui.add_space(20.0);
                ui.separator();
                ui.add_space(10.0);
                if let Ok(log) = GLOBAL_LOG.lock() {
                    if let Some(msg) = log.last() {
                        // ui.label(msg);
                        ui.label(egui::RichText::new(msg).size(14.0));
                    }
                }
            });
        });

        ctx.request_repaint();
    }
}

impl VideoDownloader {
    fn start_download(&mut self) {
        let url = self.input.clone();
        log_message(format!("Fetching Information..."));
        let (progress_sender, progress_receiver) = mpsc::channel(100);
        let (cancel_sender, _) = broadcast::channel(1);

        self.is_downloading = true;
        self.progress_receiver = Some(progress_receiver);
        self.cancel_sender = Some(cancel_sender.clone());

        let handle = tokio::spawn(async move {
            let _ = download(url, progress_sender, cancel_sender).await;
        });

        self.download_handle = Some(handle);
    }

    fn cancel_download(&mut self) {
        if let Some(cancel_sender) = &self.cancel_sender {
            let _ = cancel_sender.send(());
        }
        self.is_downloading = false;
        self.progress = 0.0;
        if let Some(handle) = self.download_handle.take() {
            handle.abort();
        }
        // Temp fix for race condition
        thread::sleep(Duration::from_millis(1000));
        match delete_all_subfolders(SAVE_PATH) {
            Ok(_) => println!("successfully deleted temp files"),
            Err(e) => eprintln!("{}", e),
        }
        println!("Download cancelled");
        log_message(format!("Download cancelled"));
    }

    fn open_downloads_folder(&mut self) {
        let path = Path::new(SAVE_PATH);

        if !path.exists() {
            return log_message(format!("Downloads folder not found!"));
        }

        let _status = if cfg!(target_os = "windows") {
            Command::new("explorer")
        } else if cfg!(target_os = "linux") {
            Command::new("xdg-open")
        } else if cfg!(target_os = "macos") {
            Command::new("open")
        } else {
            return log_message(format!("Unsupported operating system"));
        }
        .arg(path)
        .status()
        .map_err(|e| format!("Failed to open folder: {}", e));
    }
}

async fn download(
    url: String,
    progress_sender: mpsc::Sender<f32>,
    cancel_sender: broadcast::Sender<()>,
) -> Result<(), Box<dyn std::error::Error>> {
    let f_url = if url.starts_with("https://") && url.contains(".com") {
        url.clone()
    } else {
        format!("https://missav.com/{}", url)
    };
    let uuid = get_uuid(&f_url).await?;
    let playlist_url = format!("{}{}{}", VIDEO_M3U8_PREFIX, uuid, VIDEO_PLAYLIST_SUFFIX);
    let playlist = ureq::get(&playlist_url).call()?.into_string()?;

    let lines: Vec<&str> = playlist.lines().collect();
    let resolution = lines.last().unwrap().split('/').next().unwrap();
    let m3u8_url = format!("{}{}/{}", VIDEO_M3U8_PREFIX, uuid, lines.last().unwrap());

    let off_max_str = ureq::get(&m3u8_url).call()?.into_string()?;
    let lines: Vec<&str> = off_max_str.lines().collect();
    let off_max = lines[lines.len() - 2];
    let re = Regex::new(r"\d+")?;
    let digit = re
        .captures(off_max)
        .and_then(|captures| captures.get(0))
        .and_then(|matched| matched.as_str().parse::<i32>().ok())
        .ok_or("Failed to extract count")?;

    let movie_name = url.rsplit('/').next().unwrap().to_string();
    make_folders(&movie_name)?;

    let num_cpus = get_num_cpus();
    let intervals = split_integer_into_intervals(digit + 1, num_cpus);

    reset_counter();

    let result = download_jpegs_frames(
        intervals,
        &uuid,
        resolution,
        &movie_name,
        digit,
        progress_sender,
        cancel_sender,
    )
    .await;

    if result.is_ok() {
        let file_path = format!("{}/{}.mp4", SAVE_PATH, movie_name);
        if !Path::new(&file_path).exists() {
            if ffmpeg_check() {
                frames_to_video_ffmpeg(&movie_name, digit)?;
            } else {
                frame_concat(&movie_name, digit)?;
            }
        }
    }

    reset_counter();
    Ok(())
}

fn ffmpeg_check() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map_or(false, |output| output.status.success())
}

fn reset_counter() {
    if let Ok(mut count) = COUNTER.lock() {
        *count = 0;
    }
}

fn get_num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

async fn get_uuid(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let res = ureq::get(url).call()?.into_string()?;
    let re = Regex::new(r"https:\\/\\/sixyik\.com\\/([^\\/]+)\\/seek\\/_0\.jpg")?;
    re.captures(&res)
        .and_then(|captures| captures.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| "Failed to match uuid.".into())
}

fn make_folders(name: &str) -> io::Result<()> {
    let path = format!("{}/{}", SAVE_PATH, name);
    fs::create_dir_all(&path)?;
    println!("Created directory: {}", path);
    log_message(format!("Created directory: {}", path));
    Ok(())
}

fn delete_all_subfolders(folder_path: &str) -> std::io::Result<()> {
    let path = Path::new(folder_path);

    if !path.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let item_path = entry.path();

        if item_path.is_dir() {
            fs::remove_dir_all(item_path)?;
        }
    }

    Ok(())
}

fn split_integer_into_intervals(integer: i32, n: usize) -> Vec<(i32, i32)> {
    let interval_size = integer / n as i32;
    let remainder = integer % n as i32;

    let mut intervals: Vec<(i32, i32)> = (0..n)
        .map(|i| (i as i32 * interval_size, (i as i32 + 1) * interval_size))
        .collect();

    if let Some(last) = intervals.last_mut() {
        last.1 += remainder;
    }
    intervals
}

async fn download_jpegs_frames(
    intervals: Vec<(i32, i32)>,
    uuid: &str,
    resolution: &str,
    movie_name: &str,
    video_offset_max: i32,
    progress_sender: mpsc::Sender<f32>,
    cancel_sender: broadcast::Sender<()>,
) -> Result<(), String> {
    log_message(format!("Download Started...Please wait"));
    let total_frames = video_offset_max + 1;
    let mut handles = vec![];

    for (start, end) in intervals {
        let uuid = uuid.to_string();
        let resolution = resolution.to_string();
        let movie_name = movie_name.to_string();
        let progress_sender = progress_sender.clone();
        let mut cancel_receiver = cancel_sender.subscribe();

        let handle = tokio::spawn(async move {
            for i in start..end {
                if cancel_receiver.try_recv().is_ok() {
                    return Ok::<(), String>(());
                }

                let url_tmp = format!("https://surrit.com/{}/{}/video{}.jpeg", uuid, resolution, i);

                if let Some(content) = request_with_retry(&url_tmp) {
                    let file_path = format!("{}/{}/video{}.jpeg", SAVE_PATH, movie_name, i);
                    if let Some(parent) = Path::new(&file_path).parent() {
                        fs::create_dir_all(parent).expect("Failed to create directories");
                    }

                    if File::create(&file_path)
                        .and_then(|mut file| file.write_all(&content))
                        .is_err()
                    {
                        eprintln!("Failed to write file: {}", file_path);
                        continue;
                    }

                    let progress;
                    {
                        let mut count = COUNTER.lock().unwrap();
                        *count += 1;
                        progress = *count as f32 / total_frames as f32;
                    }
                    let _ = progress_sender.send(progress).await;
                } else {
                    eprintln!("Failed to download: {}", url_tmp);
                    log_message(format!("Failed to download: {}", url_tmp));
                }
            }
            Ok(())
        });
        handles.push(handle);
    }

    for handle in handles {
        handle
            .await
            .map_err(|e| format!("Thread failed: {:?}", e))??;
    }

    Ok(())
}

fn request_with_retry(url: &str) -> Option<Vec<u8>> {
    let max_retries = 5;
    let delay = Duration::from_secs(2);

    for _ in 0..max_retries {
        match ureq::get(url).call() {
            Ok(res) if res.status() == 200 => {
                let mut bytes = Vec::new();
                if res.into_reader().read_to_end(&mut bytes).is_ok() {
                    return Some(bytes);
                }
            }
            _ => thread::sleep(delay),
        }
    }
    log_message(format!("Request timed out! Check the URL"));
    None
}

fn frame_concat(name: &str, total_frames: i32) -> io::Result<()> {
    log_message(format!("Video processing started...Please wait"));
    let list_file = format!("{}/{}/list.txt", SAVE_PATH, name);
    let mut list_txt = File::create(&list_file)?;

    for i in 0..=total_frames {
        let file_path = format!("{}/{}/video{}.jpeg", SAVE_PATH, name, i);
        if Path::new(&file_path).exists() {
            writeln!(list_txt, "{}/{}/video{}.jpeg", SAVE_PATH, name, i)?;
        }
    }
    let out_file_name = format!("{}/{}.mp4", SAVE_PATH, name);
    let list = CString::new(list_file).expect("Failed to parse list file");
    let out = CString::new(out_file_name).expect("Failed to parse file name");
    unsafe {
        let result = fconcat(list.as_ptr(), out.as_ptr());
        if result == 0 {
            match delete_all_subfolders(SAVE_PATH) {
                Ok(_) => println!("successfully deleted temp files"),
                Err(e) => eprintln!("{}", e),
            }
            println!("FFmpeg execution completed.");
            log_message(format!("SUCCESS!!! Output Saved to : {}", SAVE_PATH));
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                format!("video processing failed for movie: {}", name),
            ))
        }
    }
}

fn frames_to_video_ffmpeg(name: &str, total_frames: i32) -> io::Result<()> {
    log_message(format!(
        "FFMPEG Detected.\nVideo processing started...Please wait"
    ));
    let list_file = format!("{}/{}/list.txt", SAVE_PATH, name);
    let mut list_txt = File::create(&list_file)?;

    for i in 0..=total_frames {
        let file_path = format!("{}/{}/video{}.jpeg", SAVE_PATH, name, i);
        if Path::new(&file_path).exists() {
            writeln!(list_txt, "file 'video{}.jpeg'", i)?;
        }
    }

    // if !Path::new(OUT_PATH).exists() {
    //     match fs::create_dir_all(OUT_PATH) {
    //         Ok(_) => println!("Created directory: {}", OUT_PATH),
    //         Err(e) => eprintln!("Failed to create directory: {}", e),
    //     }
    // } else {
    //     println!("Directory already exists: {}", OUT_PATH);
    // }

    let out_file_name = format!("{}/{}.mp4", SAVE_PATH, name);

    let ffmpeg_path = if cfg!(target_os = "windows") {
        "bin/ffmpeg.exe"
    } else if cfg!(target_os = "linux") {
        "bin/ffmpeg"
    } else {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "Unsupported operating system",
        ));
    };

    let status = Command::new(ffmpeg_path)
        .args([
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            &list_file,
            "-c",
            "copy",
            &out_file_name,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(winapi::um::winbase::CREATE_NO_WINDOW)
        .status()?;

    if status.success() {
        match delete_all_subfolders(SAVE_PATH) {
            Ok(_) => println!("successfully deleted temp files"),
            Err(e) => eprintln!("{}", e),
        }
        println!("FFmpeg execution completed.");
        log_message(format!("SUCCESS!!! Output Saved to : {}", SAVE_PATH));
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("FFmpeg execution failed for movie: {}", name),
        ))
    }
}

#[tokio::main]
async fn main() -> Result<(), eframe::Error> {
    // let options = eframe::NativeOptions::default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([640.0, 320.0])
            .with_min_inner_size([640.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native(
        "MAd Beta 0.5.0",
        options,
        Box::new(|_cc| Ok(Box::new(VideoDownloader::default()))),
    )
}
