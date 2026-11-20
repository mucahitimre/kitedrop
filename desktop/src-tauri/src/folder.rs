//! Folders are sent as one `.zip`, like AirDrop does (owner request, 2026-11-20). The archive is
//! written to the app's `outgoing` cache folder; the core seals its own copy right away, so archives
//! older than a day are removed at start.

use std::fs::File;
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Archives older than this are removed by [`clean`].
const KEEP: Duration = Duration::from_secs(24 * 3600);

/// Zips `dir` into `<outgoing>/<random>/<dir name>.zip`. Entries are stored under the folder's own
/// name, so unzipping recreates the folder. Symbolic links are kept as links (app bundles need
/// them) and never followed. `progress(done, total)` gets the bytes of file content read so far
/// (after every chunk; the caller throttles).
pub fn zip_dir(
    dir: &Path,
    outgoing: &Path,
    mut progress: impl FnMut(u64, u64),
) -> io::Result<PathBuf> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "folder".to_owned());
    let hex: String = kitedrop_core::random::<8>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let out_dir = outgoing.join(hex);
    std::fs::create_dir_all(&out_dir)?;
    let out = out_dir.join(format!("{name}.zip"));
    let mut zip = ZipWriter::new(BufWriter::new(File::create(&out)?));
    let base = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut count = Count {
        done: 0,
        total: content_size(dir),
        progress: &mut progress,
    };
    add(&mut zip, dir, &name, base, &mut count)?;
    zip.finish()?
        .into_inner()
        .map_err(|e| e.into_error())?
        .sync_all()?;
    Ok(out)
}

struct Count<'a> {
    done: u64,
    total: u64,
    progress: &'a mut dyn FnMut(u64, u64),
}

/// Bytes of regular files under `path` that go into the archive (links not followed,
/// `.DS_Store` skipped as in [`add`]): the total for progress.
fn content_size(path: &Path) -> u64 {
    if path.file_name().is_some_and(|n| n == ".DS_Store") {
        return 0;
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => m.len(),
        Ok(m) if m.is_dir() => std::fs::read_dir(path)
            .map(|rd| rd.flatten().map(|e| content_size(&e.path())).sum())
            .unwrap_or(0),
        _ => 0,
    }
}

fn add(
    zip: &mut ZipWriter<BufWriter<File>>,
    path: &Path,
    entry: &str,
    base: SimpleFileOptions,
    count: &mut Count,
) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    // Modification time in local time (what ZIP readers expect); dates before 1980 are left out.
    let base = match meta
        .modified()
        .ok()
        .map(|t| chrono::DateTime::<chrono::Local>::from(t).naive_local())
        .and_then(|t| zip::DateTime::try_from(t).ok())
    {
        Some(t) => base.last_modified_time(t),
        None => base,
    };
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(path)?;
        zip.add_symlink(entry, target.to_string_lossy(), base)?;
    } else if meta.is_dir() {
        zip.add_directory(format!("{entry}/"), base)?;
        let mut children: Vec<_> = std::fs::read_dir(path)?.collect::<io::Result<_>>()?;
        children.sort_by_key(|c| c.file_name());
        for c in children {
            let child = c.file_name().to_string_lossy().into_owned();
            if child == ".DS_Store" {
                continue;
            }
            add(zip, &c.path(), &format!("{entry}/{child}"), base, count)?;
        }
    } else if meta.is_file() {
        zip.start_file(entry, base.large_file(meta.len() >= u64::from(u32::MAX)))?;
        let mut file = File::open(path)?;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = io::Read::read(&mut file, &mut buf)?;
            if n == 0 {
                break;
            }
            io::Write::write_all(zip, &buf[..n])?;
            count.done += n as u64;
            (count.progress)(count.done, count.total.max(count.done));
        }
    }
    Ok(())
}

/// Removes archives older than a day.
pub fn clean(outgoing: &Path) {
    let Ok(entries) = std::fs::read_dir(outgoing) else {
        return;
    };
    let now = SystemTime::now();
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).unwrap_or_default() > KEEP);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn zips_a_folder_under_its_own_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Photos");
        std::fs::create_dir_all(dir.join("trip/empty")).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        std::fs::write(dir.join("trip/b.bin"), vec![7u8; 100_000]).unwrap();
        std::fs::write(dir.join(".DS_Store"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("a.txt", dir.join("link")).unwrap();

        let mut last = (0, 0);
        let out = zip_dir(&dir, &tmp.path().join("outgoing"), |d, t| last = (d, t)).unwrap();
        // Progress ends at exactly the content size (5 + 100 000 bytes; .DS_Store is not included).
        assert_eq!(last, (100_005, 100_005));
        assert_eq!(out.file_name().unwrap(), "Photos.zip");
        let mut z = zip::ZipArchive::new(File::open(&out).unwrap()).unwrap();
        let mut names: Vec<String> = z.file_names().map(|n| n.unwrap().into_owned()).collect();
        names.sort();
        let mut want = vec![
            "Photos/",
            "Photos/a.txt",
            "Photos/trip/",
            "Photos/trip/b.bin",
            "Photos/trip/empty/",
        ];
        if cfg!(unix) {
            want.push("Photos/link");
        }
        want.sort();
        assert_eq!(names, want);
        let mut s = String::new();
        z.by_name("Photos/a.txt")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert_eq!(s, "hello");
        let mut b = Vec::new();
        z.by_name("Photos/trip/b.bin")
            .unwrap()
            .read_to_end(&mut b)
            .unwrap();
        assert_eq!(b, vec![7u8; 100_000]);
        #[cfg(unix)]
        assert!(z.by_name("Photos/link").unwrap().is_symlink());
        // Modification times are kept (not the 1980 default).
        let t = z.by_name("Photos/a.txt").unwrap().last_modified().unwrap();
        assert!(t.year() >= 2024);
    }
}
