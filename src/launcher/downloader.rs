use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

pub struct DownloadEntry {
    pub url: String,
    pub dest_path: PathBuf,
    pub display_name: String,
}

#[derive(Clone, Default)]
pub struct DownloadProgress {
    /// 0-based index within the full 28-file manifest (for the stats "N / 28" display).
    pub file_index: usize,
    pub total_files: usize,
    pub display_name: String,
    /// Bytes downloaded in this session only (excludes pre-existing file content).
    pub bytes_done: u64,
    /// Total bytes that need to be downloaded in this session (0 during check phase).
    pub bytes_total: u64,
    /// Bytes received so far for the current file (including any pre-existing bytes from resume).
    pub current_file_bytes: u64,
    /// Full size of the current file (0 if unknown).
    pub current_file_size: u64,
    pub is_complete: bool,
    pub error: Option<String>,
}

struct FileTask {
    global_idx: usize,
    url: String,
    dest_path: PathBuf,
    display_name: String,
    remote_size: u64,
    resume_from: u64,
}

fn all_entries(dest_root: &Path) -> Vec<DownloadEntry> {
    let mut entries = Vec::new();

    for lat in 45u32..=49 {
        for lon in 9u32..=13 {
            let name = format!("Copernicus_DSM_COG_10_N{lat:02}_00_E{lon:03}_00_DEM");
            entries.push(DownloadEntry {
                url: format!("https://copernicus-dem-30m.s3.amazonaws.com/{name}/{name}.tif"),
                dest_path: dest_root
                    .join("tiles")
                    .join(&name)
                    .join(format!("{name}.tif")),
                display_name: format!("{name}.tif"),
            });
        }
    }

    entries.push(DownloadEntry {
        url: "https://data.bev.gv.at/download/DGM/Hoehenraster/DGM_R5.tif".to_string(),
        dest_path: dest_root.join("tiles").join("big_size").join("DGM_R5.tif"),
        display_name: "DGM_R5.tif".to_string(),
    });

    entries.push(DownloadEntry {
        url: "https://data.bev.gv.at/download/ALS/DTM/20240915/ALS_DTM_CRS3035RES50000mN2650000E4400000.tif".to_string(),
        dest_path: dest_root
            .join("tiles")
            .join("big_size")
            .join("CRS3035RES50000mN2650000E4400000.tif"),
        display_name: "CRS3035RES50000mN2650000E4400000.tif".to_string(),
    });

    entries.push(DownloadEntry {
        url: "https://data.bev.gv.at/download/ALS/DTM/20240915/ALS_DTM_CRS3035RES50000mN2650000E4450000.tif".to_string(),
        dest_path: dest_root
            .join("tiles")
            .join("big_size")
            .join("CRS3035RES50000mN2650000E4450000.tif"),
        display_name: "CRS3035RES50000mN2650000E4450000.tif".to_string(),
    });

    entries
}

fn head_content_length(url: &str) -> u64 {
    ureq::head(url)
        .call()
        .ok()
        .and_then(|r| r.header("content-length").and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

pub fn begin_download(dest_root: PathBuf) -> (mpsc::Receiver<DownloadProgress>, Arc<AtomicBool>) {
    let (tx, rx) = mpsc::channel::<DownloadProgress>();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = Arc::clone(&cancel);

    std::thread::spawn(move || {
        let entries = all_entries(&dest_root);
        let total_files = entries.len();

        // Show the card immediately before any network activity.
        if tx
            .send(DownloadProgress {
                total_files,
                display_name: "Checking files…".to_string(),
                ..DownloadProgress::default()
            })
            .is_err()
        {
            return;
        }

        // ── Phase 1: check which files need downloading ────────────────────
        let mut tasks: Vec<FileTask> = Vec::new();

        for (i, entry) in entries.iter().enumerate() {
            if cancel_clone.load(Ordering::Relaxed) {
                return;
            }
            let remote_size = head_content_length(&entry.url);
            let local_size = std::fs::metadata(&entry.dest_path)
                .map(|m| m.len())
                .unwrap_or(0);

            if remote_size > 0 && local_size == remote_size {
                // Already complete — send a brief progress tick so the card updates.
                let _ = tx.send(DownloadProgress {
                    file_index: i,
                    total_files,
                    display_name: entry.display_name.clone(),
                    current_file_bytes: remote_size,
                    current_file_size: remote_size,
                    ..DownloadProgress::default()
                });
                continue;
            }

            let resume_from = if local_size > 0 && local_size < remote_size {
                local_size
            } else {
                0
            };
            tasks.push(FileTask {
                global_idx: i,
                url: entry.url.clone(),
                dest_path: entry.dest_path.clone(),
                display_name: entry.display_name.clone(),
                remote_size,
                resume_from,
            });
        }

        if tasks.is_empty() {
            let _ = tx.send(DownloadProgress {
                file_index: total_files.saturating_sub(1),
                total_files,
                display_name: "All files ready".to_string(),
                is_complete: true,
                ..DownloadProgress::default()
            });
            return;
        }

        // ── Phase 2: download only what's missing ─────────────────────────
        // bytes_done / bytes_total tracks only the actual download work in this
        // session — pre-existing bytes are excluded so the ring goes 0 → 100 %
        // cleanly without the 100 → 65 % regression caused by skipped files.
        let bytes_total: u64 = tasks
            .iter()
            .map(|t| t.remote_size.saturating_sub(t.resume_from))
            .sum();
        let mut bytes_done: u64 = 0;

        for task in &tasks {
            if cancel_clone.load(Ordering::Relaxed) {
                break;
            }

            if let Some(parent) = task.dest_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    let _ = tx.send(DownloadProgress {
                        file_index: task.global_idx,
                        total_files,
                        display_name: task.display_name.clone(),
                        bytes_done,
                        bytes_total,
                        current_file_bytes: task.resume_from,
                        current_file_size: task.remote_size,
                        is_complete: false,
                        error: Some(format!("mkdir failed: {e}")),
                    });
                    return;
                }
            }

            let file = if task.resume_from > 0 {
                std::fs::OpenOptions::new()
                    .write(true)
                    .append(true)
                    .open(&task.dest_path)
            } else {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&task.dest_path)
            };

            let file = match file {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx.send(DownloadProgress {
                        file_index: task.global_idx,
                        total_files,
                        display_name: task.display_name.clone(),
                        bytes_done,
                        bytes_total,
                        current_file_bytes: task.resume_from,
                        current_file_size: task.remote_size,
                        is_complete: false,
                        error: Some(format!("open failed: {e}")),
                    });
                    return;
                }
            };

            let resp = if task.resume_from > 0 {
                ureq::get(&task.url)
                    .set("Range", &format!("bytes={}-", task.resume_from))
                    .call()
            } else {
                ureq::get(&task.url).call()
            };

            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(DownloadProgress {
                        file_index: task.global_idx,
                        total_files,
                        display_name: task.display_name.clone(),
                        bytes_done,
                        bytes_total,
                        current_file_bytes: task.resume_from,
                        current_file_size: task.remote_size,
                        is_complete: false,
                        error: Some(format!("GET failed: {e}")),
                    });
                    return;
                }
            };

            let current_file_size = if resp.status() == 206 {
                let remaining: u64 = resp
                    .header("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                task.resume_from + remaining
            } else {
                resp.header("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(task.remote_size)
            };

            let mut current_file_bytes = task.resume_from;
            let mut reader = resp.into_reader();
            let mut writer = BufWriter::new(file);
            let mut chunk = vec![0u8; 65536];

            loop {
                if cancel_clone.load(Ordering::Relaxed) {
                    let _ = writer.flush();
                    let _ = tx.send(DownloadProgress {
                        file_index: task.global_idx,
                        total_files,
                        display_name: task.display_name.clone(),
                        bytes_done,
                        bytes_total,
                        current_file_bytes,
                        current_file_size,
                        is_complete: false,
                        error: Some("Cancelled".to_string()),
                    });
                    return;
                }

                let n = match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => {
                        let _ = tx.send(DownloadProgress {
                            file_index: task.global_idx,
                            total_files,
                            display_name: task.display_name.clone(),
                            bytes_done,
                            bytes_total,
                            current_file_bytes,
                            current_file_size,
                            is_complete: false,
                            error: Some(format!("read error: {e}")),
                        });
                        return;
                    }
                };

                if let Err(e) = writer.write_all(&chunk[..n]) {
                    let _ = tx.send(DownloadProgress {
                        file_index: task.global_idx,
                        total_files,
                        display_name: task.display_name.clone(),
                        bytes_done,
                        bytes_total,
                        current_file_bytes,
                        current_file_size,
                        is_complete: false,
                        error: Some(format!("write error: {e}")),
                    });
                    return;
                }

                current_file_bytes += n as u64;
                bytes_done += n as u64;

                if tx
                    .send(DownloadProgress {
                        file_index: task.global_idx,
                        total_files,
                        display_name: task.display_name.clone(),
                        bytes_done,
                        bytes_total,
                        current_file_bytes,
                        current_file_size,
                        is_complete: false,
                        error: None,
                    })
                    .is_err()
                {
                    return;
                }
            }

            let _ = writer.flush();
        }

        let _ = tx.send(DownloadProgress {
            file_index: total_files.saturating_sub(1),
            total_files,
            display_name: "All files ready".to_string(),
            bytes_done,
            bytes_total,
            is_complete: true,
            ..DownloadProgress::default()
        });
    });

    (rx, cancel)
}
