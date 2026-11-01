/*

Glk Streams
===========

Copyright (c) 2025 Dannii Willis
MIT licenced
https://github.com/curiousdannii/remglk-rs

*/

use std::cmp::{max, min};
use std::ffi::CString;

use enum_dispatch::enum_dispatch;

use super::*;

const GLK_NULL: u32 = 0;

/** A `Stream` wrapped in an GlkObject (`Arc<Mutex>>`) */
pub type GlkStreamShared = GlkObject<GlkStream>;
pub type GlkStreamMetadata = GlkObjectMetadata<GlkStream>;
pub type GlkStreamWeak = GlkObjectWeak<GlkStream>;

#[enum_dispatch]
pub enum GlkStream {
    ArrayBacked(ArrayBackedStream),
    FileStream(FileStream),
    Null(NullStream),
    Window(WindowStream),
}

impl Default for GlkStream {
    fn default() -> Self {
        GlkStream::Null(NullStream::default())
    }
}

impl GlkObjectClass for GlkStream {
    fn get_object_class_id() -> u32 {
        1
    }
}

/** A stream operation */
pub enum StreamOperation<'a> {
    GetBuffer(&'a mut GlkBufferMut<'a>),
    /** Read a byte, pass true for unicode */
    GetChar(bool),
    GetLine(&'a mut GlkBufferMut<'a>),
    GetPosition,
    PutBuffer(&'a GlkBuffer<'a>),
    PutChar(u32),
    PutString(&'a str, Option<u32>),
    SetPosition(SeekMode, i32),
}
use StreamOperation::*;

#[enum_dispatch(GlkStream)]
pub trait StreamOperations {
    fn close(&self) -> StreamResultCounts {
        StreamResultCounts {
            read_count: 0,
            write_count: self.write_count() as u32,
        }
    }
    fn do_operation(&mut self, op: StreamOperation) -> GlkResult<'_, i32>;
    fn file_path(&self) -> GlkResult<'_, &CString> {
        Err(NotFileStream)
    }
    fn write_count(&self) -> usize;
}

/** A fixed-length stream based on a buffer (a boxed slice).
    ArrayBackedStreams are used for memory and resource streams, and are the basis of file streams.
*/
pub struct ArrayBackedStream {
    pub(crate) binary: bool,
    pub(crate) buf: GlkOwnedBuffer,
    /** Whether we need to check if we should expand the active buffer region before writing */
    expandable: bool,
    fmode: FileMode,
    /** The length of the active region of the buffer.
        This can be shorter than the actual length of the buffer in a `filemode_Write` memory stream.
        Expanding filestreams is handled differently, see below.
        See https://github.com/iftechfoundation/ifarchive-if-specs/issues/8
    */
    len: usize,
    path: Option<CString>,
    pos: usize,
    read_count: usize,
    pub(crate) resource: Option<ResourceStreamInfo>,
    write_count: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct ResourceStreamInfo {
    pub binary: bool,
    pub filenum: u32,
}

impl ArrayBackedStream {
    pub fn new(
        buf: GlkOwnedBuffer,
        fmode: FileMode,
        fileref: Option<&GlkFileRef>,
    ) -> ArrayBackedStream {
        let buf_len = buf.len();
        let path = fileref.map(|fileref| CString::new(&fileref.path[..]).unwrap());
        ArrayBackedStream {
            binary: fileref.map_or(false, |fileref| fileref.binary),
            buf,
            expandable: fmode == FileMode::Write,
            fmode,
            len: match fmode {
                FileMode::Write => 0,
                _ => buf_len,
            },
            path,
            pos: 0,
            read_count: 0,
            resource: None,
            write_count: 0,
        }
    }

    fn expand(&mut self, increase: usize) {
        self.len = min(self.pos + increase, self.buf.len());
        if self.len == self.buf.len() {
            self.expandable = false;
        }
    }

    pub fn take_buffer(&mut self) -> GlkOwnedBuffer {
        mem::take(&mut self.buf)
    }
}

impl StreamOperations for ArrayBackedStream {
    fn close(&self) -> StreamResultCounts {
        StreamResultCounts {
            read_count: self.read_count as u32,
            write_count: self.write_count as u32,
        }
    }

    fn do_operation(&mut self, op: StreamOperation) -> GlkResult<'_, i32> {
        // Check file mode first
        match &op {
            GetBuffer(_) | GetChar(_) | GetLine(_) => {
                if let FileMode::Write | FileMode::WriteAppend = self.fmode {
                    return Err(ReadFromWriteOnly);
                }
            }
            PutBuffer(_) | PutChar(_) | PutString(_, _) => {
                if let FileMode::Read = self.fmode {
                    return Err(WriteToReadOnly);
                }
            }
            _ => {}
        };

        match op {
            GetBuffer(buf) => {
                let read_length = min(buf.len(), self.len - self.pos);
                if read_length == 0 {
                    return Ok(0);
                }
                self.buf.copy_to_buffer(self.pos, buf, 0, read_length);
                self.pos += read_length;
                self.read_count += read_length;
                Ok(read_length as i32)
            }
            GetChar(uni) => {
                if self.pos < self.len {
                    let ch = self.buf.get_u32(self.pos);
                    self.pos += 1;
                    self.read_count += 1;
                    return Ok(if !uni && ch > MAX_LATIN1 {
                        QUESTION_MARK
                    } else {
                        ch
                    } as i32);
                }
                Ok(-1)
            }
            GetLine(buf) => {
                let read_length: isize =
                    min(buf.len() as isize - 1, (self.len - self.pos) as isize);
                if read_length < 0 {
                    return Ok(0);
                }
                let mut i: usize = 0;
                while i < read_length as usize {
                    let ch = self.buf.get_u32(self.pos);
                    self.pos += 1;
                    buf.set_u32(i, ch);
                    i += 1;
                    if ch == 10 {
                        break;
                    }
                }
                buf.set_u32(i, GLK_NULL);
                self.read_count += i;
                Ok(i as i32)
            }
            GetPosition => Ok(self.pos as i32),
            PutBuffer(buf) => {
                let buf_len = buf.len();
                self.write_count += buf_len;
                if self.pos + buf_len > self.len && self.expandable {
                    self.expand(buf_len);
                }
                let write_length = min(buf_len, self.len - self.pos);
                if write_length > 0 {
                    self.buf.copy_from_buffer(self.pos, buf, 0, write_length);
                    self.pos += write_length;
                }
                Ok(0)
            }
            PutChar(ch) => {
                self.write_count += 1;
                if self.pos == self.len && self.expandable {
                    self.expand(1);
                }
                if self.pos < self.len {
                    self.buf.set_u32(self.pos, ch);
                    self.pos += 1;
                }
                Ok(0)
            }
            PutString(str, _style) => {
                let buf: GlkOwnedBuffer = Into::<GlkOwnedBuffer>::into(str);
                self.do_operation(PutBuffer(&(&buf).into()))
            }
            SetPosition(mode, pos) => {
                let new_pos: i32 = match mode {
                    SeekMode::Current => self.pos as i32 + pos,
                    SeekMode::End => self.len as i32 + pos,
                    SeekMode::Start => pos,
                };
                self.pos = new_pos.clamp(0, self.len as i32) as usize;
                Ok(0)
            }
        }
    }

    fn file_path(&self) -> GlkResult<'_, &CString> {
        self.path.as_ref().ok_or(NotFileStream)
    }

    fn write_count(&self) -> usize {
        self.write_count
    }
}

/** Writable FileStreams are based on array backed streams, but can grow in length.
    Read-only file streams just use an ArrayBackedStream directly.
*/
pub struct FileStream {
    binary: bool,
    pub changed: bool,
    pub path: String,
    str: ArrayBackedStream,
}

impl FileStream {
    pub fn new(fileref: &GlkFileRef, buf: GlkOwnedBuffer, fmode: FileMode) -> FileStream {
        debug_assert!(fmode != FileMode::Read);
        let str = ArrayBackedStream::new(buf, fmode, Some(fileref));
        FileStream {
            binary: fileref.binary,
            changed: false,
            path: fileref.path.clone(),
            str,
        }
    }

    fn expand(&mut self, increase: usize) {
        let end_pos = self.str.pos + increase;
        let mut max_len = self.str.buf.len();
        if end_pos > max_len {
            // Expand the vec by at least 100
            max_len += max(end_pos - max_len, 100);
            self.str.buf.resize(max_len);
        }
        self.str.expand(increase);
    }

    pub fn to_file_buffer(&self) -> Box<[u8]> {
        self.str.buf.to_file_buffer(self.binary, self.str.len)
    }
}

impl StreamOperations for FileStream {
    fn close(&self) -> StreamResultCounts {
        self.str.close()
    }

    fn do_operation(&mut self, op: StreamOperation) -> GlkResult<'_, i32> {
        match op {
            PutBuffer(buf) => {
                self.changed = true;
                if self.str.pos + buf.len() > self.str.len {
                    self.expand(buf.len());
                }
            }
            PutChar(_) => {
                self.changed = true;
                if self.str.pos == self.str.len {
                    self.expand(1);
                }
            }
            PutString(str, _) => {
                let buf: GlkOwnedBuffer = Into::<GlkOwnedBuffer>::into(str);
                return self.do_operation(PutBuffer(&(&buf).into()));
            }
            SetPosition(mode, pos) => {
                // Despite the Glk spec saying that it's illegal to specify a position after the end of a file (5.4) this is needed by Bocfel, and seemingly supported by all libc based Glk interpreters, so we might need to expand first
                // See https://github.com/iftechfoundation/ifarchive-if-specs/issues/17
                let new_pos = match mode {
                    SeekMode::Current => self.str.pos as i32 + pos,
                    SeekMode::End => self.str.len as i32 + pos,
                    SeekMode::Start => pos,
                } as usize;
                if new_pos > self.str.len {
                    self.expand(new_pos - self.str.len);
                }
            }
            _ => {}
        }
        self.str.do_operation(op)
    }

    fn file_path(&self) -> GlkResult<'_, &CString> {
        self.str.file_path()
    }

    fn write_count(&self) -> usize {
        self.str.write_count
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum StreamDataState {
    Window {
        window: u32,
        write_count: u32,
    },
    Memory {
        buffer: Option<SavedBuffer>,
        fmode: FileMode,
        len: usize,
        position: usize,
        read_count: u32,
        write_count: u32,
    },
    File {
        path: String,
        binary: bool,
        unicode: bool,
        fmode: FileMode,
        position: usize,
        read_count: u32,
        write_count: u32,
    },
    Resource {
        buffer: SavedBuffer,
        binary: bool,
        filenum: u32,
        position: usize,
        read_count: u32,
        write_count: u32,
    },
}

impl StreamDataState {
    pub(crate) fn snapshot<F>(
        str: &GlkStream,
        array_disprock: Option<DispatchRock>,
        mut locate: F,
    ) -> Result<Self, AutosaveError>
    where
        F: FnMut(&GlkOwnedBuffer, Option<DispatchRock>) -> Result<SavedBuffer, AutosaveError>,
    {
        match str {
            GlkStream::Window(data) => Ok(Self::Window {
                window: weak_stream_window_id(data)?,
                write_count: usize_to_u32(data.write_count)?,
            }),
            GlkStream::Null(data) => Ok(Self::Memory {
                buffer: None,
                fmode: FileMode::Write,
                len: 0,
                position: 0,
                read_count: 0,
                write_count: usize_to_u32(data.write_count)?,
            }),
            GlkStream::FileStream(data) => Ok(Self::File {
                path: data.path.clone(),
                binary: data.binary,
                unicode: matches!(data.str.buf, GlkOwnedBuffer::U32(_)),
                fmode: data.str.fmode,
                position: data.str.pos,
                read_count: usize_to_u32(data.str.read_count)?,
                write_count: usize_to_u32(data.str.write_count)?,
            }),
            GlkStream::ArrayBacked(data) if data.path.is_some() => Ok(Self::File {
                path: data
                    .path
                    .as_ref()
                    .unwrap()
                    .to_str()
                    .map_err(|_| {
                        AutosaveError::MalformedGraph("file stream path is not UTF-8".into())
                    })?
                    .to_string(),
                binary: data.binary,
                unicode: matches!(data.buf, GlkOwnedBuffer::U32(_)),
                fmode: data.fmode,
                position: data.pos,
                read_count: usize_to_u32(data.read_count)?,
                write_count: usize_to_u32(data.write_count)?,
            }),
            GlkStream::ArrayBacked(data) => {
                if let Some(resource) = data.resource {
                    Ok(Self::Resource {
                        buffer: SavedBuffer::from_buffer(&data.buf, 0),
                        binary: resource.binary,
                        filenum: resource.filenum,
                        position: data.pos,
                        read_count: usize_to_u32(data.read_count)?,
                        write_count: usize_to_u32(data.write_count)?,
                    })
                } else {
                    Ok(Self::Memory {
                        buffer: Some(locate(&data.buf, array_disprock)?),
                        fmode: data.fmode,
                        len: data.len,
                        position: data.pos,
                        read_count: usize_to_u32(data.read_count)?,
                        write_count: usize_to_u32(data.write_count)?,
                    })
                }
            }
        }
    }

    pub(crate) fn validate(&self) -> Result<(), AutosaveError> {
        match self {
            Self::Memory {
                buffer: Some(buffer),
                fmode,
                len,
                position,
                ..
            } => {
                if *fmode == FileMode::WriteAppend || *len > buffer.len() || *position > *len {
                    return Err(AutosaveError::MalformedGraph(
                        "memory stream mode, position, or length is invalid".into(),
                    ));
                }
            }
            Self::Memory {
                buffer: None,
                fmode,
                len,
                position,
                read_count,
                ..
            } => {
                if *fmode != FileMode::Write || *len != 0 || *position != 0 || *read_count != 0 {
                    return Err(AutosaveError::MalformedGraph(
                        "null memory stream has inconsistent state".into(),
                    ));
                }
            }
            Self::File { position, .. } => {
                if *position > i32::MAX as usize {
                    return Err(AutosaveError::LimitExceeded(
                        "file stream position exceeds the Glk seek range".into(),
                    ));
                }
            }
            Self::Resource {
                buffer, position, ..
            } => {
                if *position > buffer.len() {
                    return Err(AutosaveError::MalformedGraph(
                        "resource stream position exceeds its buffer".into(),
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl ArrayBackedStream {
    pub(crate) fn restore_state(
        &mut self,
        len: usize,
        position: usize,
        read_count: u32,
        write_count: u32,
    ) {
        self.len = len;
        self.pos = position;
        self.read_count = read_count as usize;
        self.write_count = write_count as usize;
        self.expandable = self.fmode == FileMode::Write && self.len < self.buf.len();
    }
}

impl FileStream {
    pub(crate) fn restore_state(&mut self, position: usize, read_count: u32, write_count: u32) {
        self.str
            .restore_state(self.str.len, position, read_count, write_count);
    }
}

impl WindowStream {
    pub(crate) fn restore_write_count(&mut self, write_count: u32) {
        self.write_count = write_count as usize;
    }
}

impl NullStream {
    pub(crate) fn restore_write_count(&mut self, write_count: u32) {
        self.write_count = write_count as usize;
    }
}

fn weak_stream_window_id(data: &WindowStream) -> Result<u32, AutosaveError> {
    data.win
        .upgrade()
        .map(|win| win.lock().unwrap().id)
        .ok_or_else(|| AutosaveError::MalformedGraph("dangling window stream link".into()))
}

fn usize_to_u32(value: usize) -> Result<u32, AutosaveError> {
    value
        .try_into()
        .map_err(|_| AutosaveError::LimitExceeded("stream counter exceeds uint32".into()))
}

/** A NullStream is only used for a memory stream with no buffer */
#[derive(Default)]
pub struct NullStream {
    write_count: usize,
}

impl StreamOperations for NullStream {
    fn do_operation(&mut self, op: StreamOperation) -> GlkResult<'_, i32> {
        match op {
            PutBuffer(buf) => self.write_count += buf.len(),
            PutChar(_) => self.write_count += 1,
            PutString(str, _) => self.write_count += str.chars().count(),
            _ => {}
        };
        Ok(if let GetChar(_) = op { -1 } else { 0 })
    }

    fn write_count(&self) -> usize {
        self.write_count
    }
}

/** A window stream */
#[derive(Default)]
pub struct WindowStream {
    pub win: GlkWindowWeak,
    write_count: usize,
}

impl WindowStream {
    pub fn new(win: &GlkWindowShared) -> Self {
        WindowStream {
            win: win.downgrade(),
            ..Default::default()
        }
    }
}

impl StreamOperations for WindowStream {
    fn do_operation(&mut self, op: StreamOperation) -> GlkResult<'_, i32> {
        if let GetChar(_) = &op {
            return Ok(-1);
        }
        if let PutBuffer(_) | PutChar(_) | PutString(_, _) = op {
            let win: GlkWindowShared = (&self.win).into();
            let mut win = win.lock().unwrap();
            if let Some(TextInputType::Line) = win.input.text_input_type {
                return Err(PendingLineInput);
            }
            match op {
                PutBuffer(buf) => {
                    self.write_count += buf.len();
                    win.put_string(&buf.to_string(), None);
                }
                PutChar(ch) => {
                    self.write_count += 1;
                    win.put_string(&char::from_u32(ch).unwrap().to_string(), None);
                }
                PutString(str, style) => {
                    self.write_count += str.chars().count();
                    win.put_string(str, style);
                }
                _ => {}
            };
            if let Some(str) = &win.echostr {
                let str: GlkStreamShared = str.into();
                str.lock().unwrap().do_operation(op)?;
            }
        }
        Ok(0)
    }

    fn write_count(&self) -> usize {
        self.write_count
    }
}
