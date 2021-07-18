use std::ffi::OsString;
use std::fmt;
use std::future::Future;
use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

#[cfg(unix)]
use std::os::unix::fs::{DirEntryExt as _, OpenOptionsExt as _};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt as _;

use crate::future::future;
use crate::future::stream::Stream;
use crate::future::swap::{AsyncRead, AsyncSeek, AsyncWrite, AsyncWriteExt};
use crate::platform::lock::Mutex;
use crate::ready;
use crate::runtime::blocking::{unblock, Unblock};

pub use std::fs::{FileType, Metadata, Permissions};

#[allow(dead_code)]
pub async fn canonicalize<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::canonicalize(&path)).await
}

#[allow(dead_code)]
pub async fn copy<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<u64> {
    let src = src.as_ref().to_owned();
    let dst = dst.as_ref().to_owned();
    unblock(move || std::fs::copy(&src, &dst)).await
}

#[allow(dead_code)]
pub async fn create_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::create_dir(&path)).await
}

#[allow(dead_code)]
pub async fn create_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::create_dir_all(&path)).await
}

#[allow(dead_code)]
pub async fn hard_link<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    let src = src.as_ref().to_owned();
    let dst = dst.as_ref().to_owned();
    unblock(move || std::fs::hard_link(&src, &dst)).await
}

#[allow(dead_code)]
pub async fn metadata<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::metadata(path)).await
}

#[allow(dead_code)]
pub async fn read<P: AsRef<Path>>(path: P) -> io::Result<Vec<u8>> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::read(&path)).await
}

#[allow(dead_code)]
pub async fn read_dir<P: AsRef<Path>>(path: P) -> io::Result<ReadDir> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::read_dir(&path).map(|inner| ReadDir(State::Idle(Some(inner))))).await
}

pub struct ReadDir(State);

enum State {
    Idle(Option<std::fs::ReadDir>),
    Busy(future::Boxed<(std::fs::ReadDir, Option<io::Result<std::fs::DirEntry>>)>),
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadDir").finish()
    }
}

impl Stream for ReadDir {
    type Item = io::Result<DirEntry>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match &mut self.0 {
                State::Idle(opt) => {
                    let mut inner = opt.take().unwrap();

                    self.0 = State::Busy(Box::pin(unblock(move || {
                        let next = inner.next();
                        (inner, next)
                    })));
                }
                State::Busy(task) => {
                    let (inner, opt) = crate::ready!(task.as_mut().poll(cx));
                    self.0 = State::Idle(Some(inner));
                    return Poll::Ready(opt.map(|res| res.map(|inner| DirEntry(Arc::new(inner)))));
                }
            }
        }
    }
}

pub struct DirEntry(Arc<std::fs::DirEntry>);

#[allow(dead_code)]
impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.0.path().into()
    }

    #[allow(dead_code)]
    pub async fn metadata(&self) -> io::Result<Metadata> {
        let inner = self.0.clone();
        unblock(move || inner.metadata()).await
    }

    #[allow(dead_code)]
    pub async fn file_type(&self) -> io::Result<FileType> {
        let inner = self.0.clone();
        unblock(move || inner.file_type()).await
    }

    pub fn file_name(&self) -> OsString {
        self.0.file_name()
    }
}

impl fmt::Debug for DirEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DirEntry").field(&self.path()).finish()
    }
}

impl Clone for DirEntry {
    fn clone(&self) -> Self {
        DirEntry(self.0.clone())
    }
}

#[cfg(unix)]
impl unix::DirEntryExt for DirEntry {
    fn ino(&self) -> u64 {
        self.0.ino()
    }
}

#[allow(dead_code)]
pub async fn read_link<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::read_link(&path)).await
}

#[allow(dead_code)]
pub async fn read_to_string<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::read_to_string(&path)).await
}

#[allow(dead_code)]
pub async fn remove_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::remove_dir(&path)).await
}

#[allow(dead_code)]
pub async fn remove_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::remove_dir_all(&path)).await
}

#[allow(dead_code)]
pub async fn remove_file<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::remove_file(&path)).await
}

#[allow(dead_code)]
pub async fn rename<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    let src = src.as_ref().to_owned();
    let dst = dst.as_ref().to_owned();
    unblock(move || std::fs::rename(&src, &dst)).await
}

#[allow(dead_code)]
pub async fn set_permissions<P: AsRef<Path>>(path: P, perm: Permissions) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::set_permissions(path, perm)).await
}

#[allow(dead_code)]
pub async fn symlink_metadata<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    let path = path.as_ref().to_owned();
    unblock(move || std::fs::symlink_metadata(path)).await
}

#[allow(dead_code)]
pub async fn write<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, contents: C) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    let contents = contents.as_ref().to_owned();
    unblock(move || std::fs::write(&path, contents)).await
}

#[derive(Debug, Default)]
pub struct DirBuilder {
    recursive: bool,

    #[cfg(unix)]
    mode: Option<u32>,
}

#[allow(dead_code)]
impl DirBuilder {
    pub fn new() -> DirBuilder {
        #[cfg(not(unix))]
        let builder = DirBuilder { recursive: false };

        #[cfg(unix)]
        let builder = DirBuilder {
            recursive: false,
            mode: None,
        };

        builder
    }

    pub fn recursive(&mut self, recursive: bool) -> &mut Self {
        self.recursive = recursive;
        self
    }

    pub fn create<P: AsRef<Path>>(&self, path: P) -> impl Future<Output = io::Result<()>> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(self.recursive);

        #[cfg(unix)]
        {
            if let Some(mode) = self.mode {
                std::os::unix::fs::DirBuilderExt::mode(&mut builder, mode);
            }
        }

        let path = path.as_ref().to_owned();
        unblock(move || builder.create(path))
    }
}

#[cfg(unix)]
impl unix::DirBuilderExt for DirBuilder {
    fn mode(&mut self, mode: u32) -> &mut Self {
        self.mode = Some(mode);
        self
    }
}

pub struct File {
    file: Arc<std::fs::File>,

    unblock: Mutex<Unblock<ArcFile>>,

    read_pos: Option<io::Result<u64>>,

    is_dirty: bool,
}

#[allow(dead_code)]
impl File {
    fn new(inner: std::fs::File, is_dirty: bool) -> File {
        let file = Arc::new(inner);
        let unblock = Mutex::new(Unblock::new(ArcFile(file.clone())));
        let read_pos = None;
        File {
            file,
            unblock,
            read_pos,
            is_dirty,
        }
    }

    pub async fn open<P: AsRef<Path>>(path: P) -> io::Result<File> {
        let path = path.as_ref().to_owned();
        let file = unblock(move || std::fs::File::open(&path)).await?;
        Ok(File::new(file, false))
    }

    pub async fn create<P: AsRef<Path>>(path: P) -> io::Result<File> {
        let path = path.as_ref().to_owned();
        let file = unblock(move || std::fs::File::create(&path)).await?;
        Ok(File::new(file, false))
    }

    pub async fn sync_all(&self) -> io::Result<()> {
        let mut inner = self.unblock.lock().await;
        inner.flush().await?;
        let file = self.file.clone();
        unblock(move || file.sync_all()).await
    }

    pub async fn sync_data(&self) -> io::Result<()> {
        let mut inner = self.unblock.lock().await;
        inner.flush().await?;
        let file = self.file.clone();
        unblock(move || file.sync_data()).await
    }

    pub async fn set_len(&self, size: u64) -> io::Result<()> {
        let mut inner = self.unblock.lock().await;
        inner.flush().await?;
        let file = self.file.clone();
        unblock(move || file.set_len(size)).await
    }

    pub async fn metadata(&self) -> io::Result<Metadata> {
        let file = self.file.clone();
        unblock(move || file.metadata()).await
    }

    pub async fn set_permissions(&self, perm: Permissions) -> io::Result<()> {
        let file = self.file.clone();
        unblock(move || file.set_permissions(perm)).await
    }

    fn poll_reposition(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(Ok(read_pos)) = self.read_pos {
            ready!(Pin::new(self.unblock.get_mut()).poll_seek(cx, SeekFrom::Start(read_pos)))?;
        }
        self.read_pos = None;
        Poll::Ready(Ok(()))
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.file.fmt(f)
    }
}

impl From<std::fs::File> for File {
    fn from(inner: std::fs::File) -> File {
        File::new(inner, true)
    }
}

#[cfg(unix)]
impl std::os::unix::io::FromRawFd for File {
    unsafe fn from_raw_fd(raw: std::os::unix::io::RawFd) -> File {
        File::from(std::fs::File::from_raw_fd(raw))
    }
}

#[cfg(windows)]
impl std::os::windows::io::FromRawHandle for File {
    unsafe fn from_raw_handle(raw: std::os::windows::io::RawHandle) -> File {
        File::from(std::fs::File::from_raw_handle(raw))
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsRawFd for File {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.file.as_raw_fd()
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsRawHandle for File {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        self.file.as_raw_handle()
    }
}

impl AsyncRead for File {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.read_pos.is_none() {
            self.read_pos = Some(ready!(self.as_mut().poll_seek(cx, SeekFrom::Current(0))));
        }

        let n = ready!(Pin::new(self.unblock.get_mut()).poll_read(cx, buf))?;

        if let Some(Ok(pos)) = self.read_pos.as_mut() {
            *pos += n as u64;
        }

        Poll::Ready(Ok(n))
    }
}

impl AsyncWrite for File {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        ready!(self.poll_reposition(cx))?;
        self.is_dirty = true;
        Pin::new(self.unblock.get_mut()).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.is_dirty {
            ready!(Pin::new(self.unblock.get_mut()).poll_flush(cx))?;
            self.is_dirty = false;
        }
        Poll::Ready(Ok(()))
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.unblock.get_mut()).poll_close(cx)
    }
}

impl AsyncSeek for File {
    fn poll_seek(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        pos: SeekFrom,
    ) -> Poll<io::Result<u64>> {
        ready!(self.poll_reposition(cx))?;
        Pin::new(self.unblock.get_mut()).poll_seek(cx, pos)
    }
}

struct ArcFile(Arc<std::fs::File>);

impl io::Read for ArcFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self.0).read(buf)
    }
}

impl io::Write for ArcFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self.0).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self.0).flush()
    }
}

impl io::Seek for ArcFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        (&*self.0).seek(pos)
    }
}

#[derive(Clone, Debug)]
pub struct OpenOptions(std::fs::OpenOptions);

#[allow(dead_code)]
impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions(std::fs::OpenOptions::new())
    }

    pub fn read(&mut self, read: bool) -> &mut OpenOptions {
        self.0.read(read);
        self
    }

    pub fn write(&mut self, write: bool) -> &mut OpenOptions {
        self.0.write(write);
        self
    }

    pub fn append(&mut self, append: bool) -> &mut OpenOptions {
        self.0.append(append);
        self
    }

    pub fn truncate(&mut self, truncate: bool) -> &mut OpenOptions {
        self.0.truncate(truncate);
        self
    }

    pub fn create(&mut self, create: bool) -> &mut OpenOptions {
        self.0.create(create);
        self
    }

    pub fn create_new(&mut self, create_new: bool) -> &mut OpenOptions {
        self.0.create_new(create_new);
        self
    }

    pub fn open<P: AsRef<Path>>(&self, path: P) -> impl Future<Output = io::Result<File>> {
        let path = path.as_ref().to_owned();
        let options = self.0.clone();
        async move {
            let file = unblock(move || options.open(path)).await?;
            Ok(File::new(file, false))
        }
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
impl unix::OpenOptionsExt for OpenOptions {
    fn mode(&mut self, mode: u32) -> &mut Self {
        self.0.mode(mode);
        self
    }

    fn custom_flags(&mut self, flags: i32) -> &mut Self {
        self.0.custom_flags(flags);
        self
    }
}

#[cfg(windows)]
impl windows::OpenOptionsExt for OpenOptions {
    fn access_mode(&mut self, access: u32) -> &mut Self {
        self.0.access_mode(access);
        self
    }

    fn share_mode(&mut self, val: u32) -> &mut Self {
        self.0.share_mode(val);
        self
    }

    fn custom_flags(&mut self, flags: u32) -> &mut Self {
        self.0.custom_flags(flags);
        self
    }

    fn attributes(&mut self, val: u32) -> &mut Self {
        self.0.attributes(val);
        self
    }

    fn security_qos_flags(&mut self, flags: u32) -> &mut Self {
        self.0.security_qos_flags(flags);
        self
    }
}

#[cfg(unix)]
#[allow(dead_code)]
pub mod unix {
    use super::*;

    pub use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

    pub async fn symlink<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
        let src = src.as_ref().to_owned();
        let dst = dst.as_ref().to_owned();
        unblock(move || std::os::unix::fs::symlink(&src, &dst)).await
    }

    pub trait DirBuilderExt {
        fn mode(&mut self, mode: u32) -> &mut Self;
    }

    pub trait DirEntryExt {
        fn ino(&self) -> u64;
    }

    pub trait OpenOptionsExt {
        fn mode(&mut self, mode: u32) -> &mut Self;

        fn custom_flags(&mut self, flags: i32) -> &mut Self;
    }
}

#[cfg(windows)]
pub mod windows {
    use super::*;

    pub use std::os::windows::fs::MetadataExt;

    pub async fn symlink_dir<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
        let src = src.as_ref().to_owned();
        let dst = dst.as_ref().to_owned();
        unblock(move || std::os::windows::fs::symlink_dir(&src, &dst)).await
    }

    pub async fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
        let src = src.as_ref().to_owned();
        let dst = dst.as_ref().to_owned();
        unblock(move || std::os::windows::fs::symlink_file(&src, &dst)).await
    }

    pub trait OpenOptionsExt {
        fn access_mode(&mut self, access: u32) -> &mut Self;

        fn share_mode(&mut self, val: u32) -> &mut Self;

        fn custom_flags(&mut self, flags: u32) -> &mut Self;

        fn attributes(&mut self, val: u32) -> &mut Self;

        fn security_qos_flags(&mut self, flags: u32) -> &mut Self;
    }
}
