/*

Library-state autosave/autorestore
==================================

Copyright (c) 2026 Dannii Willis
MIT licenced
https://github.com/curiousdannii/remglk-rs

*/

use std::collections::{HashMap, HashSet};
use std::slice;
use std::sync::Weak;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::*;

pub const LIBRARY_STATE_FORMAT: &str = "remglk-rs-library-state";
pub const LIBRARY_STATE_VERSION: u32 = 1;
pub const MAX_LIBRARY_STATE_BYTES: usize = 12 * 1024 * 1024;
pub const RESTORED_EVENT_TYPE: u32 = 0xFFFFFFFE;

pub(crate) const MAX_GRID_CELLS: usize = 1024 * 1024;
pub(crate) const MAX_WINDOW_DIMENSION: usize = 4096;
const MAX_BUFFER_ELEMENTS: usize = 4 * 1024 * 1024;
const MAX_OBJECTS: usize = 4096;
const MAX_PATH_BYTES: usize = 4096;

#[derive(Debug, Error)]
pub enum AutosaveError {
    #[error("library-state JSON exceeds the {MAX_LIBRARY_STATE_BYTES}-byte limit")]
    InputTooLarge,
    #[error("unsupported library-state format or version")]
    UnsupportedVersion,
    #[error("invalid library-state JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("library-state limit exceeded: {0}")]
    LimitExceeded(String),
    #[error("malformed library-state graph: {0}")]
    MalformedGraph(String),
    #[error("retained-array autorestore registry is not installed")]
    MissingArrayRegistry,
    #[error("retained-array restore callback failed")]
    ArrayRestoreFailed,
    #[error("saved file could not be reopened: {0}")]
    FileUnavailable(String),
    #[error("saved resource {0} is unavailable in the current Blorb")]
    ResourceUnavailable(u32),
    #[error("saved resource {0} differs from the current Blorb")]
    ResourceChanged(u32),
    #[error("library-state stream I/O failed: {0}")]
    StreamIo(String),
    #[error("library state has already been consumed")]
    Consumed,
    #[error("live Glk teardown failed: {0}")]
    Teardown(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "encoding", rename_all = "snake_case")]
pub enum SavedBuffer {
    U8 { address: i64, data: Vec<u8> },
    U32 { address: i64, data: Vec<u32> },
}

impl SavedBuffer {
    pub(crate) fn from_buffer(buffer: &GlkOwnedBuffer, address: i64) -> Self {
        match buffer {
            GlkOwnedBuffer::U8(data) => Self::U8 {
                address,
                data: data.to_vec(),
            },
            GlkOwnedBuffer::U32(data) => Self::U32 {
                address,
                data: data.to_vec(),
            },
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::U8 { data, .. } => data.len(),
            Self::U32 { data, .. } => data.len(),
        }
    }

    fn address(&self) -> i64 {
        match self {
            Self::U8 { address, .. } | Self::U32 { address, .. } => *address,
        }
    }

    fn validate(&self) -> Result<(), AutosaveError> {
        if self.len() > MAX_BUFFER_ELEMENTS {
            return Err(AutosaveError::LimitExceeded(
                "retained buffer has too many elements".into(),
            ));
        }
        if self.address() < 0 || self.address() > u32::MAX as i64 {
            return Err(AutosaveError::MalformedGraph(
                "retained buffer address is outside the VM address space".into(),
            ));
        }
        Ok(())
    }

    fn matches(&self, buffer: &GlkOwnedBuffer) -> bool {
        match (self, buffer) {
            (Self::U8 { data: saved, .. }, GlkOwnedBuffer::U8(current)) => {
                saved.as_slice() == current.as_ref()
            }
            (Self::U32 { data: saved, .. }, GlkOwnedBuffer::U32(current)) => {
                saved.as_slice() == current.as_ref()
            }
            _ => false,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WindowState {
    tag: u32,
    rock: u32,
    wintype: WindowType,
    wbox: WindowBox,
    parent_tag: Option<u32>,
    stream_tag: u32,
    echo_stream_tag: Option<u32>,
    input: InputUpdate,
    uni_char_input: bool,
    data: WindowDataState,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StreamState {
    tag: u32,
    rock: u32,
    data: StreamDataState,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FileRefState {
    tag: u32,
    rock: u32,
    path: String,
    binary: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LibraryStateSnapshot {
    format: String,
    version: u32,
    generation: u32,
    metrics: NormalisedMetrics,
    support: SupportedFeatures,
    windows: Vec<WindowState>,
    streams: Vec<StreamState>,
    filerefs: Vec<FileRefState>,
    timer_interval: u32,
    root_window_tag: Option<u32>,
    current_stream_tag: Option<u32>,
    extra_state: Option<Value>,
}

pub struct LibraryState {
    snapshot: Option<LibraryStateSnapshot>,
}

impl LibraryState {
    pub fn from_json(data: &[u8]) -> Result<Self, AutosaveError> {
        if data.len() > MAX_LIBRARY_STATE_BYTES {
            return Err(AutosaveError::InputTooLarge);
        }
        let snapshot: LibraryStateSnapshot = serde_json::from_slice(data)?;
        snapshot.validate()?;
        Ok(Self {
            snapshot: Some(snapshot),
        })
    }

    pub fn extra_state(&self) -> Option<&Value> {
        self.snapshot.as_ref()?.extra_state.as_ref()
    }
}

struct RestoredGraph {
    windows: HashMap<u32, GlkWindowShared>,
    streams: HashMap<u32, GlkStreamShared>,
    filerefs: HashMap<u32, GlkFileRefShared>,
}

impl LibraryStateSnapshot {
    fn validate(&self) -> Result<(), AutosaveError> {
        if self.format != LIBRARY_STATE_FORMAT || self.version != LIBRARY_STATE_VERSION {
            return Err(AutosaveError::UnsupportedVersion);
        }
        if self.windows.len() > MAX_OBJECTS
            || self.streams.len() > MAX_OBJECTS
            || self.filerefs.len() > MAX_OBJECTS
        {
            return Err(AutosaveError::LimitExceeded("too many Glk objects".into()));
        }
        validate_metrics(&self.metrics)?;

        unique_tags(self.windows.iter().map(|window| window.tag), "window")?;
        let stream_tags = unique_tags(self.streams.iter().map(|stream| stream.tag), "stream")?;
        unique_tags(self.filerefs.iter().map(|fileref| fileref.tag), "fileref")?;
        let windows: HashMap<u32, &WindowState> = self
            .windows
            .iter()
            .map(|window| (window.tag, window))
            .collect();

        for window in &self.windows {
            if !stream_tags.contains(&window.stream_tag) || window.input.id != window.tag {
                return Err(AutosaveError::MalformedGraph(
                    "window stream or input tag is inconsistent".into(),
                ));
            }
            if let Some(parent) = window.parent_tag {
                let parent = windows.get(&parent).ok_or_else(|| {
                    AutosaveError::MalformedGraph("window parent is missing".into())
                })?;
                let Some((child1, child2, _)) = parent.data.pair_links() else {
                    return Err(AutosaveError::MalformedGraph(
                        "window parent is not a pair".into(),
                    ));
                };
                if child1 != window.tag && child2 != window.tag {
                    return Err(AutosaveError::MalformedGraph(
                        "pair does not link back to its child".into(),
                    ));
                }
            }
            if let Some(echo) = window.echo_stream_tag {
                if !stream_tags.contains(&echo) {
                    return Err(AutosaveError::MalformedGraph(
                        "window echo stream is missing".into(),
                    ));
                }
            }
            if !matches!(
                (window.wintype, &window.data),
                (WindowType::Blank, WindowDataState::Blank)
                    | (WindowType::Pair, WindowDataState::Pair { .. })
                    | (WindowType::Buffer, WindowDataState::Buffer(_))
                    | (WindowType::Graphics, WindowDataState::Graphics(_))
                    | (WindowType::Grid, WindowDataState::Grid(_))
            ) {
                return Err(AutosaveError::MalformedGraph(
                    "window type and data variant differ".into(),
                ));
            }
            if matches!(window.wintype, WindowType::Blank | WindowType::Pair)
                && (window.input.text_input_type.is_some()
                    || window.input.hyperlink
                    || window.input.mouse)
            {
                return Err(AutosaveError::MalformedGraph(
                    "blank or pair window has pending input".into(),
                ));
            }
            validate_box(window.wbox)?;
            window.data.validate()?;
            if let Some(buffer) = window.data.line_input_buffer() {
                buffer.validate()?;
                if window.input.text_input_type != Some(TextInputType::Line) {
                    return Err(AutosaveError::MalformedGraph(
                        "retained input buffer has no line request".into(),
                    ));
                }
            } else if window.input.text_input_type == Some(TextInputType::Line) {
                return Err(AutosaveError::MalformedGraph(
                    "line request has no retained input buffer".into(),
                ));
            }
            if let Some((child1, child2, key)) = window.data.pair_links() {
                if child1 == child2 || child1 == window.tag || child2 == window.tag {
                    return Err(AutosaveError::MalformedGraph(
                        "pair children are invalid".into(),
                    ));
                }
                for child in [child1, child2] {
                    let child = windows.get(&child).ok_or_else(|| {
                        AutosaveError::MalformedGraph("pair child is missing".into())
                    })?;
                    if child.parent_tag != Some(window.tag) {
                        return Err(AutosaveError::MalformedGraph(
                            "pair child has an invalid back-link".into(),
                        ));
                    }
                }
                let mut ancestor = Some(key);
                let mut seen = HashSet::new();
                let mut descendant = false;
                while let Some(tag) = ancestor {
                    if !seen.insert(tag) {
                        return Err(AutosaveError::MalformedGraph("window parent cycle".into()));
                    }
                    if tag == window.tag {
                        descendant = true;
                        break;
                    }
                    ancestor = windows.get(&tag).and_then(|window| window.parent_tag);
                }
                if !descendant
                    || windows
                        .get(&key)
                        .map_or(true, |key| key.wintype == WindowType::Pair)
                {
                    return Err(AutosaveError::MalformedGraph(
                        "pair key is not a non-pair descendant".into(),
                    ));
                }
            }
        }

        match (self.windows.is_empty(), self.root_window_tag) {
            (true, None) => {}
            (false, Some(root)) => {
                let root = windows.get(&root).ok_or_else(|| {
                    AutosaveError::MalformedGraph("root window is missing".into())
                })?;
                if root.parent_tag.is_some() {
                    return Err(AutosaveError::MalformedGraph(
                        "root window has a parent".into(),
                    ));
                }
                let mut reachable = HashSet::new();
                let mut pending = vec![root.tag];
                while let Some(tag) = pending.pop() {
                    if !reachable.insert(tag) {
                        return Err(AutosaveError::MalformedGraph(
                            "window tree contains a cycle".into(),
                        ));
                    }
                    let window = windows.get(&tag).unwrap();
                    if let Some((child1, child2, _)) = window.data.pair_links() {
                        pending.push(child1);
                        pending.push(child2);
                    }
                }
                if reachable.len() != self.windows.len() {
                    return Err(AutosaveError::MalformedGraph(
                        "window tree has unreachable objects".into(),
                    ));
                }
            }
            _ => {
                return Err(AutosaveError::MalformedGraph(
                    "root window and window list disagree".into(),
                ))
            }
        }

        let mut window_stream_count = 0;
        for stream in &self.streams {
            stream.data.validate()?;
            if let StreamDataState::Window { window, .. } = stream.data {
                window_stream_count += 1;
                let owner = windows.get(&window).ok_or_else(|| {
                    AutosaveError::MalformedGraph("window stream owner is missing".into())
                })?;
                if owner.stream_tag != stream.tag {
                    return Err(AutosaveError::MalformedGraph(
                        "window and stream back-links differ".into(),
                    ));
                }
            }
            if let StreamDataState::Memory {
                buffer: Some(buffer),
                ..
            } = &stream.data
            {
                buffer.validate()?;
            }
            if let StreamDataState::Resource { buffer, .. } = &stream.data {
                buffer.validate()?;
            }
            if let StreamDataState::File { path, .. } = &stream.data {
                validate_path(path)?;
            }
        }
        if window_stream_count != self.windows.len() {
            return Err(AutosaveError::MalformedGraph(
                "window stream count does not match window count".into(),
            ));
        }
        if self
            .current_stream_tag
            .is_some_and(|tag| !stream_tags.contains(&tag))
        {
            return Err(AutosaveError::MalformedGraph(
                "current stream is missing".into(),
            ));
        }
        for fileref in &self.filerefs {
            validate_path(&fileref.path)?;
        }
        Ok(())
    }
}

fn unique_tags(
    tags: impl Iterator<Item = u32>,
    class: &str,
) -> Result<HashSet<u32>, AutosaveError> {
    let mut result = HashSet::new();
    for tag in tags {
        if tag == 0 || tag == u32::MAX || !result.insert(tag) {
            return Err(AutosaveError::MalformedGraph(format!(
                "duplicate, zero, or overflowing {class} tag"
            )));
        }
    }
    Ok(result)
}

fn validate_path(path: &str) -> Result<(), AutosaveError> {
    if path.is_empty() || path.len() > MAX_PATH_BYTES || path.as_bytes().contains(&0) {
        return Err(AutosaveError::MalformedGraph(
            "invalid or excessive file path".into(),
        ));
    }
    Ok(())
}

fn validate_box(wbox: WindowBox) -> Result<(), AutosaveError> {
    if ![wbox.bottom, wbox.left, wbox.right, wbox.top]
        .into_iter()
        .all(f64::is_finite)
        || wbox.left < 0.0
        || wbox.top < 0.0
        || wbox.right < wbox.left
        || wbox.bottom < wbox.top
    {
        return Err(AutosaveError::MalformedGraph(
            "invalid window bounds".into(),
        ));
    }
    Ok(())
}

fn validate_metrics(metrics: &NormalisedMetrics) -> Result<(), AutosaveError> {
    let values = [
        metrics.buffercharheight,
        metrics.buffercharwidth,
        metrics.buffermarginx,
        metrics.buffermarginy,
        metrics.graphicsmarginx,
        metrics.graphicsmarginy,
        metrics.gridcharheight,
        metrics.gridcharwidth,
        metrics.gridmarginx,
        metrics.gridmarginy,
        metrics.height,
        metrics.inspacingx,
        metrics.inspacingy,
        metrics.width,
    ];
    if !values
        .into_iter()
        .all(|value| value.is_finite() && value >= 0.0)
        || metrics.buffercharheight <= 0.0
        || metrics.buffercharwidth <= 0.0
        || metrics.gridcharheight <= 0.0
        || metrics.gridcharwidth <= 0.0
    {
        return Err(AutosaveError::MalformedGraph(
            "invalid display metrics".into(),
        ));
    }
    Ok(())
}

impl<S> GlkApi<S>
where
    S: Default + GlkSystem,
{
    pub fn save_library_state(
        &self,
        omitstream: Option<&GlkStreamShared>,
        extra_state: Option<Value>,
    ) -> Result<Vec<u8>, AutosaveError> {
        let omitstream = omitstream.map(|stream| stream.as_ptr());
        let mut windows: Vec<WindowState> = self
            .windows
            .iter()
            .map(|window| {
                let window = lock!(window);
                Ok(WindowState {
                    tag: window.id,
                    rock: window.rock,
                    wintype: window.wintype,
                    wbox: window.wbox,
                    parent_tag: window
                        .parent
                        .as_ref()
                        .map(|parent| weak_tag(parent))
                        .transpose()?,
                    stream_tag: weak_tag(&window.str)?,
                    echo_stream_tag: window
                        .echostr
                        .as_ref()
                        .map(|stream| weak_tag(stream))
                        .transpose()?,
                    input: window.input.clone(),
                    uni_char_input: window.uni_char_input,
                    data: WindowDataState::snapshot(
                        &window.data,
                        window.array_disprock,
                        |buffer, rock| self.locate_saved_buffer(buffer, rock),
                    )?,
                })
            })
            .collect::<Result<_, AutosaveError>>()?;
        windows.sort_by_key(|window| window.tag);

        let mut streams: Vec<StreamState> = self
            .streams
            .iter()
            .filter(|stream| Some(stream.as_ptr()) != omitstream)
            .map(|stream| {
                let stream = lock!(stream);
                Ok(StreamState {
                    tag: stream.id,
                    rock: stream.rock,
                    data: StreamDataState::snapshot(
                        &stream,
                        stream.array_disprock,
                        |buffer, rock| self.locate_saved_buffer(buffer, rock),
                    )?,
                })
            })
            .collect::<Result<_, AutosaveError>>()?;
        streams.sort_by_key(|stream| stream.tag);

        let mut filerefs: Vec<FileRefState> = self
            .filerefs
            .iter()
            .map(|fileref| {
                let fileref = lock!(fileref);
                FileRefState {
                    tag: fileref.id,
                    rock: fileref.rock,
                    path: fileref.path.clone(),
                    binary: fileref.binary,
                }
            })
            .collect();
        filerefs.sort_by_key(|fileref| fileref.tag);

        let snapshot = LibraryStateSnapshot {
            format: LIBRARY_STATE_FORMAT.into(),
            version: LIBRARY_STATE_VERSION,
            generation: self.gen,
            metrics: self.metrics.clone(),
            support: self.support.clone(),
            windows,
            streams,
            filerefs,
            timer_interval: self.timer.interval,
            root_window_tag: self
                .root_window
                .as_ref()
                .map(|window| weak_tag(window))
                .transpose()?,
            current_stream_tag: self
                .current_stream
                .as_ref()
                .map(|stream| weak_tag(stream))
                .transpose()?,
            extra_state,
        };
        snapshot.validate()?;
        let data = serde_json::to_vec(&snapshot)?;
        if data.len() > MAX_LIBRARY_STATE_BYTES {
            return Err(AutosaveError::InputTooLarge);
        }
        Ok(data)
    }

    pub fn read_library_state(
        &self,
        stream: &GlkStreamShared,
    ) -> Result<LibraryState, AutosaveError> {
        let mut data = Vec::new();
        let mut stream = lock!(stream);
        loop {
            let mut chunk = [0; 8192];
            let count = stream
                .do_operation(StreamOperation::GetBuffer(&mut GlkBufferMut::U8(
                    &mut chunk,
                )))
                .map_err(|error| AutosaveError::StreamIo(error.to_string()))?
                as usize;
            if count == 0 {
                break;
            }
            if data.len() + count > MAX_LIBRARY_STATE_BYTES {
                return Err(AutosaveError::InputTooLarge);
            }
            data.extend_from_slice(&chunk[..count]);
        }
        LibraryState::from_json(&data)
    }

    pub fn write_library_state(
        &self,
        stream: &GlkStreamShared,
        data: &[u8],
    ) -> Result<(), AutosaveError> {
        if data.len() > MAX_LIBRARY_STATE_BYTES {
            return Err(AutosaveError::InputTooLarge);
        }
        lock!(stream)
            .do_operation(StreamOperation::PutBuffer(&GlkBuffer::U8(data)))
            .map_err(|error| AutosaveError::StreamIo(error.to_string()))?;
        Ok(())
    }

    pub fn update_from_library_state(
        &mut self,
        state: &mut LibraryState,
    ) -> Result<(), AutosaveError> {
        let snapshot = state.snapshot.as_ref().ok_or(AutosaveError::Consumed)?;
        snapshot.validate()?;
        let graph = self.build_restored_graph(snapshot)?;
        self.teardown_for_restore()?;

        for tag in sorted_tags(&graph.windows) {
            let window = &graph.windows[&tag];
            let state = snapshot
                .windows
                .iter()
                .find(|window| window.tag == tag)
                .unwrap();
            self.windows.register_restored(window, tag, state.rock);
        }
        for tag in sorted_tags(&graph.streams) {
            let stream = &graph.streams[&tag];
            let state = snapshot
                .streams
                .iter()
                .find(|stream| stream.tag == tag)
                .unwrap();
            self.streams.register_restored(stream, tag, state.rock);
        }
        for tag in sorted_tags(&graph.filerefs) {
            let fileref = &graph.filerefs[&tag];
            let state = snapshot
                .filerefs
                .iter()
                .find(|fileref| fileref.tag == tag)
                .unwrap();
            self.filerefs.register_restored(fileref, tag, state.rock);
        }

        self.gen = snapshot.generation;
        self.metrics = snapshot.metrics.clone();
        self.support = snapshot.support.clone();
        self.root_window = snapshot
            .root_window_tag
            .map(|tag| graph.windows[&tag].downgrade());
        self.current_stream = snapshot
            .current_stream_tag
            .map(|tag| graph.streams[&tag].downgrade());
        self.buffer_window_count = snapshot
            .windows
            .iter()
            .filter(|window| window.wintype == WindowType::Buffer)
            .count() as u32;
        self.partial_inputs = None;
        self.special = None;
        self.windows_changed = true;
        self.page_margin.transmitted = Some(u32::MAX);
        self.glk_request_timer_events(0);
        self.glk_request_timer_events(snapshot.timer_interval);
        self.timer.last_interval = if snapshot.timer_interval == 0 { 1 } else { 0 };
        self.last_event_type = RESTORED_EVENT_TYPE;
        self.exited = false;
        state.snapshot = None;
        Ok(())
    }

    pub fn glkunix_get_last_event_type(&self) -> u32 {
        self.last_event_type
    }

    fn locate_saved_buffer(
        &self,
        buffer: &GlkOwnedBuffer,
        rock: Option<DispatchRock>,
    ) -> Result<SavedBuffer, AutosaveError> {
        let rock = rock.unwrap_or(DispatchRock { num: 0 });
        let mut element_size = 0;
        let address = match buffer {
            GlkOwnedBuffer::U8(data) => self
                .locate_array_callbacks_u8
                .ok_or(AutosaveError::MissingArrayRegistry)?(
                data.as_ptr(),
                data.len() as u32,
                c"&+#!Cn".as_ptr(),
                rock,
                &mut element_size,
            ),
            GlkOwnedBuffer::U32(data) => self
                .locate_array_callbacks_u32
                .ok_or(AutosaveError::MissingArrayRegistry)?(
                data.as_ptr(),
                data.len() as u32,
                c"&+#!Iu".as_ptr(),
                rock,
                &mut element_size,
            ),
        };
        let expected = if matches!(buffer, GlkOwnedBuffer::U8(_)) {
            1
        } else {
            4
        };
        if element_size != expected {
            return Err(AutosaveError::MalformedGraph(
                "array locate callback returned the wrong element size".into(),
            ));
        }
        let saved = SavedBuffer::from_buffer(buffer, address as i64);
        saved.validate()?;
        Ok(saved)
    }

    fn restore_saved_buffer(
        &self,
        saved: &SavedBuffer,
    ) -> Result<(GlkOwnedBuffer, DispatchRock), AutosaveError> {
        saved.validate()?;
        match saved {
            SavedBuffer::U8 { address, data } => {
                let restore = self
                    .restore_array_callbacks_u8
                    .ok_or(AutosaveError::MissingArrayRegistry)?;
                let mut pointer = std::ptr::null_mut();
                let rock = restore(
                    *address as c_long,
                    data.len() as u32,
                    c"&+#!Cn".as_ptr(),
                    &mut pointer,
                );
                if pointer.is_null() && !data.is_empty() {
                    return Err(AutosaveError::ArrayRestoreFailed);
                }
                let mut buffer = if data.is_empty() {
                    Vec::new().into_boxed_slice()
                } else {
                    unsafe { Box::from_raw(slice::from_raw_parts_mut(pointer, data.len())) }
                };
                buffer.copy_from_slice(data);
                Ok((GlkOwnedBuffer::U8(buffer), rock))
            }
            SavedBuffer::U32 { address, data } => {
                let restore = self
                    .restore_array_callbacks_u32
                    .ok_or(AutosaveError::MissingArrayRegistry)?;
                let mut pointer = std::ptr::null_mut();
                let rock = restore(
                    *address as c_long,
                    data.len() as u32,
                    c"&+#!Iu".as_ptr(),
                    &mut pointer,
                );
                if pointer.is_null() && !data.is_empty() {
                    return Err(AutosaveError::ArrayRestoreFailed);
                }
                let mut buffer = if data.is_empty() {
                    Vec::new().into_boxed_slice()
                } else {
                    unsafe { Box::from_raw(slice::from_raw_parts_mut(pointer, data.len())) }
                };
                buffer.copy_from_slice(data);
                Ok((GlkOwnedBuffer::U32(buffer), rock))
            }
        }
    }

    fn build_restored_graph(
        &mut self,
        snapshot: &LibraryStateSnapshot,
    ) -> Result<RestoredGraph, AutosaveError> {
        let mut streams = HashMap::new();
        for state in &snapshot.streams {
            let stream = match &state.data {
                StreamDataState::Window { write_count, .. } => {
                    let mut data = WindowStream::default();
                    data.restore_write_count(*write_count);
                    GlkObject::new(data.into())
                }
                StreamDataState::File {
                    path,
                    binary,
                    unicode,
                    fmode,
                    position,
                    read_count,
                    write_count,
                } => {
                    let usage = if *binary {
                        fileusage_BinaryMode
                    } else {
                        fileusage_TextMode
                    };
                    let fileref = GlkFileRef::new(path.clone(), usage);
                    let stream = self
                        .open_file_stream(&fileref, *fmode, *unicode)
                        .map_err(|error| {
                            AutosaveError::FileUnavailable(format!("{path}: {error}"))
                        })?
                        .ok_or_else(|| AutosaveError::FileUnavailable(path.clone()))?;
                    {
                        let mut stream = lock!(stream);
                        stream
                            .do_operation(StreamOperation::SetPosition(SeekMode::End, 0))
                            .map_err(|error| {
                                AutosaveError::FileUnavailable(format!("{path}: {error}"))
                            })?;
                        let len =
                            stream
                                .do_operation(StreamOperation::GetPosition)
                                .map_err(|error| {
                                    AutosaveError::FileUnavailable(format!("{path}: {error}"))
                                })? as usize;
                        if *position > len {
                            return Err(AutosaveError::FileUnavailable(format!(
                                "{path}: saved position exceeds file length"
                            )));
                        }
                        stream
                            .do_operation(StreamOperation::SetPosition(
                                SeekMode::Start,
                                *position as i32,
                            ))
                            .map_err(|error| {
                                AutosaveError::FileUnavailable(format!("{path}: {error}"))
                            })?;
                        match stream.deref_mut().deref_mut() {
                            GlkStream::ArrayBacked(data) => {
                                data.restore_state(len, *position, *read_count, *write_count)
                            }
                            GlkStream::FileStream(data) => {
                                data.restore_state(*position, *read_count, *write_count)
                            }
                            _ => {
                                return Err(AutosaveError::MalformedGraph(
                                    "reopened file has the wrong stream variant".into(),
                                ))
                            }
                        }
                    }
                    stream
                }
                StreamDataState::Resource {
                    buffer,
                    binary,
                    filenum,
                    position,
                    read_count,
                    write_count,
                } => {
                    let unicode = matches!(buffer, SavedBuffer::U32 { .. });
                    let stream = self
                        .open_resource_stream(*filenum, unicode)
                        .map_err(|_| AutosaveError::ResourceUnavailable(*filenum))?
                        .ok_or(AutosaveError::ResourceUnavailable(*filenum))?;
                    {
                        let mut stream = lock!(stream);
                        let GlkStream::ArrayBacked(data) = stream.deref_mut().deref_mut() else {
                            return Err(AutosaveError::MalformedGraph(
                                "reopened resource has the wrong stream variant".into(),
                            ));
                        };
                        if data.binary != *binary || !buffer.matches(&data.buf) {
                            return Err(AutosaveError::ResourceChanged(*filenum));
                        }
                        data.restore_state(data.buf.len(), *position, *read_count, *write_count);
                    }
                    stream
                }
                StreamDataState::Memory { .. } => continue,
            };
            streams.insert(state.tag, stream);
        }

        for state in &snapshot.streams {
            let StreamDataState::Memory {
                buffer,
                fmode,
                len,
                position,
                read_count,
                write_count,
            } = &state.data
            else {
                continue;
            };
            let (stream, array_disprock) = if let Some(buffer) = buffer {
                let (buffer, rock) = self.restore_saved_buffer(buffer)?;
                let mut data = ArrayBackedStream::new(buffer, *fmode, None);
                data.restore_state(*len, *position, *read_count, *write_count);
                (GlkObject::new(data.into()), Some(rock))
            } else {
                let mut data = NullStream::default();
                data.restore_write_count(*write_count);
                (GlkObject::new(data.into()), None)
            };
            lock!(stream).array_disprock = array_disprock;
            streams.insert(state.tag, stream);
        }

        let mut windows = HashMap::new();
        for state in &snapshot.windows {
            let (line_input_buffer, array_disprock) = match state.data.line_input_buffer() {
                Some(saved) => {
                    let (buffer, rock) = self.restore_saved_buffer(saved)?;
                    (Some(buffer), Some(rock))
                }
                None => (None, None),
            };
            let mut input = state.input.clone();
            input.id = state.tag;
            let window = GlkObject::new(GlkWindow {
                data: state.data.clone().restore(line_input_buffer),
                id: state.tag,
                input,
                rock: state.rock,
                str: Weak::new(),
                uni_char_input: state.uni_char_input,
                wbox: state.wbox,
                wintype: state.wintype,
                ..Default::default()
            });
            lock!(window).array_disprock = array_disprock;
            windows.insert(state.tag, window);
        }

        let filerefs = snapshot
            .filerefs
            .iter()
            .map(|state| {
                let usage = if state.binary {
                    fileusage_BinaryMode
                } else {
                    fileusage_TextMode
                };
                (
                    state.tag,
                    GlkObject::new(GlkFileRef::new(state.path.clone(), usage)),
                )
            })
            .collect();

        for state in &snapshot.windows {
            let window = &windows[&state.tag];
            let mut window = lock!(window);
            window.parent = state.parent_tag.map(|tag| windows[&tag].downgrade());
            window.str = streams[&state.stream_tag].downgrade();
            window.echostr = state.echo_stream_tag.map(|tag| streams[&tag].downgrade());
            if let Some((child1, child2, key)) = state.data.pair_links() {
                let WindowData::Pair(data) = &mut window.data else {
                    unreachable!()
                };
                data.child1 = windows[&child1].downgrade();
                data.child2 = windows[&child2].downgrade();
                data.key = windows[&key].downgrade();
            }
        }
        for state in &snapshot.streams {
            if let StreamDataState::Window { window, .. } = state.data {
                let stream = &streams[&state.tag];
                let mut stream = lock!(stream);
                let GlkStream::Window(data) = stream.deref_mut().deref_mut() else {
                    unreachable!()
                };
                data.win = windows[&window].downgrade();
            }
        }

        Ok(RestoredGraph {
            windows,
            streams,
            filerefs,
        })
    }

    fn teardown_for_restore(&mut self) -> Result<(), AutosaveError> {
        let windows: Vec<GlkWindowShared> = self.windows.iter().cloned().collect();
        for window in windows {
            let (buffer, array_disprock) = {
                let mut window = lock!(window);
                let buffer = match &mut window.data {
                    WindowData::Buffer(data) => data.line_input_buffer.take(),
                    WindowData::Grid(data) => data.line_input_buffer.take(),
                    _ => None,
                };
                (buffer, window.array_disprock)
            };
            if let Some(buffer) = buffer {
                self.unretain_array(buffer, array_disprock);
            }
            self.windows.unregister_for_restore(window);
        }

        let streams: Vec<GlkStreamShared> = self.streams.iter().cloned().collect();
        for stream in streams {
            let window_stream = matches!(lock!(stream).deref().deref(), GlkStream::Window(_));
            if window_stream {
                self.streams.unregister_for_restore(stream);
            } else {
                self.close_stream_for_restore(stream)
                    .map_err(|error| AutosaveError::Teardown(error.to_string()))?;
            }
        }
        let filerefs: Vec<GlkFileRefShared> = self.filerefs.iter().cloned().collect();
        for fileref in filerefs {
            self.filerefs.unregister_for_restore(fileref);
        }
        self.glk_request_timer_events(0);
        self.root_window = None;
        self.current_stream = None;
        Ok(())
    }
}

fn weak_tag<T>(weak: &GlkObjectWeak<T>) -> Result<u32, AutosaveError> {
    weak.upgrade()
        .map(|object| object.lock().unwrap().id)
        .ok_or_else(|| AutosaveError::MalformedGraph("dangling object link".into()))
}

fn sorted_tags<T>(objects: &HashMap<u32, T>) -> Vec<u32> {
    let mut tags: Vec<u32> = objects.keys().copied().collect();
    tags.sort_unstable();
    tags
}

#[cfg(test)]
mod tests;
