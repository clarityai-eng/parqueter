use std::rc::Rc;
use std::time::Duration;

use gpui::{prelude::*, *};

use crate::data::DataSource;

const ROW_NUM_WIDTH: f32 = 60.0;
const ROW_HEIGHT: f32 = 28.0;
const HEADER_HEIGHT: f32 = 58.0;
const FILE_HEADER_HEIGHT: f32 = 70.0; // approximate: pt_4 + text_xl + text_sm + pb_2 + border
const SCROLL_STEP: f32 = 300.0;
const SCROLLBAR_SIZE: f32 = 10.0;
const SCROLLBAR_MIN_THUMB: f32 = 20.0;

fn to_f32(p: Pixels) -> f32 {
    let v: f32 = p.into();
    v
}

const ROW_STEP: usize = 20;
const PAGE_STEP: usize = 200;

actions!(parqueter, [ScrollUp, ScrollDown, ScrollLeft, ScrollRight, PageUp, PageDown, CopyCell, OpenFile]);

pub fn register_actions(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", ScrollUp, None),
        KeyBinding::new("down", ScrollDown, None),
        KeyBinding::new("left", ScrollLeft, None),
        KeyBinding::new("right", ScrollRight, None),
        KeyBinding::new("pageup", PageUp, None),
        KeyBinding::new("pagedown", PageDown, None),
        KeyBinding::new("ctrl-c", CopyCell, None),
        KeyBinding::new("ctrl-o", OpenFile, None),
    ]);
}

pub enum AppState {
    Loaded {
        file_name: String,
        source: Rc<dyn DataSource>,
    },
    Error(String),
}

#[derive(Clone, Copy, PartialEq)]
enum DragTarget {
    Vertical,
    Horizontal,
}

pub struct RootView {
    state: AppState,
    row_num_width: f32,
    col_widths: Vec<f32>,
    col_positions: Vec<f32>,
    data_width: f32,
    focus_handle: FocusHandle,
    v_scroll_handle: UniformListScrollHandle,
    scroll_row: usize,
    h_offset: f32,
    // Scrollbar drag state
    dragging: Option<DragTarget>,
    drag_start_pos: f32,       // mouse position when drag started
    drag_start_offset: f32,    // scroll offset when drag started
    drag_idle: bool,           // true when drag paused long enough to show data
    drag_generation: usize,    // bumped on each mouse move, idle timer checks this
    selected_cell: Option<(usize, usize)>, // (row, col)
    toast: Option<String>,
    toast_generation: usize,
    // Cached layout dimensions (updated each render)
    v_track_height: f32,
    h_track_width: f32,
    v_max_offset: f32,
    h_max_offset: f32,
}

impl RootView {
    pub fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let (col_widths, row_num_width) = match &state {
            AppState::Loaded { source, .. } => {
                let widths = source.column_widths();
                let digits = format!("{}", source.row_count()).len();
                let w = (digits as f32 * 9.0 + 24.0).max(ROW_NUM_WIDTH);
                (widths, w)
            }
            _ => (vec![], ROW_NUM_WIDTH),
        };
        let mut col_positions = Vec::with_capacity(col_widths.len());
        let mut acc = 0.0f32;
        for &w in &col_widths {
            col_positions.push(acc);
            acc += w;
        }
        let data_width = acc;
        Self {
            state,
            row_num_width,
            col_widths,
            col_positions,
            data_width,
            focus_handle: cx.focus_handle(),
            v_scroll_handle: UniformListScrollHandle::new(),
            scroll_row: 0,
            h_offset: 0.0,
            dragging: None,
            drag_start_pos: 0.0,
            drag_start_offset: 0.0,
            drag_idle: false,
            drag_generation: 0,
            selected_cell: None,
            toast: None,
            toast_generation: 0,
            v_track_height: 0.0,
            h_track_width: 0.0,
            v_max_offset: 0.0,
            h_max_offset: 0.0,
        }
    }

    fn visible_col_range(&self, viewport_width: f32) -> (usize, usize, f32) {
        if self.col_positions.is_empty() {
            return (0, 0, 0.0);
        }
        let left = self.h_offset;
        let right = self.h_offset + viewport_width;

        let first = (0..self.col_widths.len())
            .find(|&i| self.col_positions[i] + self.col_widths[i] > left)
            .unwrap_or(0);

        let last = self
            .col_positions
            .partition_point(|&pos| pos < right)
            .min(self.col_widths.len());

        let spacer = self.col_positions[first] - left;
        (first, last, spacer)
    }

    fn row_count(&self) -> usize {
        match &self.state {
            AppState::Loaded { source, .. } => source.row_count(),
            _ => 0,
        }
    }

    fn scroll_up(&mut self, _: &ScrollUp, window: &mut Window, _cx: &mut Context<Self>) {
        self.scroll_row = self.scroll_row.saturating_sub(ROW_STEP);
        self.v_scroll_handle
            .scroll_to_item(self.scroll_row, ScrollStrategy::Top);
        window.refresh();
    }

    fn scroll_down(&mut self, _: &ScrollDown, window: &mut Window, _cx: &mut Context<Self>) {
        let max = self.row_count().saturating_sub(1);
        self.scroll_row = (self.scroll_row + ROW_STEP).min(max);
        self.v_scroll_handle
            .scroll_to_item(self.scroll_row, ScrollStrategy::Top);
        window.refresh();
    }

    fn page_up(&mut self, _: &PageUp, window: &mut Window, _cx: &mut Context<Self>) {
        self.scroll_row = self.scroll_row.saturating_sub(PAGE_STEP);
        self.v_scroll_handle
            .scroll_to_item(self.scroll_row, ScrollStrategy::Top);
        window.refresh();
    }

    fn page_down(&mut self, _: &PageDown, window: &mut Window, _cx: &mut Context<Self>) {
        let max = self.row_count().saturating_sub(1);
        self.scroll_row = (self.scroll_row + PAGE_STEP).min(max);
        self.v_scroll_handle
            .scroll_to_item(self.scroll_row, ScrollStrategy::Top);
        window.refresh();
    }

    const HEADER_ROW: usize = usize::MAX;
    const TOAST_DURATION_MS: u64 = 1500;

    fn show_toast(&mut self, msg: String, window: &mut Window, cx: &mut Context<Self>) {
        self.toast = Some(msg);
        self.toast_generation = self.toast_generation.wrapping_add(1);
        let generation = self.toast_generation;
        cx.spawn_in(window, async move |_this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(RootView::TOAST_DURATION_MS))
                .await;
            cx.update(|window, cx| {
                if let Some(entity) = _this.upgrade() {
                    entity.update(cx, |view, _cx| {
                        if view.toast_generation == generation {
                            view.toast = None;
                        }
                    });
                    window.refresh();
                }
            })
            .ok();
        })
        .detach();
        window.refresh();
    }

    fn open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open Parquet file".into()),
        });
        cx.spawn_in(window, async move |_this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.into_iter().next() {
                    cx.update(|window, app| {
                        if let Some(entity) = _this.upgrade() {
                            entity.update(app, |view, cx| {
                                view.load_file(&path, window, cx);
                            });
                        }
                    }).ok();
                }
            }
        }).detach();
    }

    pub(crate) fn load_file(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        use crate::data::LazyParquetSource;

        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        match LazyParquetSource::open(path) {
            Ok(source) => {
                let source: Rc<dyn DataSource> = Rc::new(source);
                let col_widths = source.column_widths();
                let mut col_positions = Vec::with_capacity(col_widths.len());
                let mut acc = 0.0f32;
                for &w in &col_widths {
                    col_positions.push(acc);
                    acc += w;
                }
                let digits = format!("{}", source.row_count()).len();
                self.row_num_width = (digits as f32 * 9.0 + 24.0).max(ROW_NUM_WIDTH);
                self.data_width = acc;
                self.col_widths = col_widths;
                self.col_positions = col_positions;
                self.state = AppState::Loaded { file_name, source };
                self.scroll_row = 0;
                self.h_offset = 0.0;
                self.selected_cell = None;
                self.v_scroll_handle = UniformListScrollHandle::new();
                self.show_toast(format!("Opened: {}", path.display()), window, cx);
            }
            Err(e) => {
                self.state = AppState::Error(e);
                window.refresh();
            }
        }
    }

    fn cell_text_for_selection(&self, row: usize, col: usize) -> Option<String> {
        let AppState::Loaded { source, .. } = &self.state else {
            return None;
        };
        if row == Self::HEADER_ROW {
            Some(source.column_name(col).to_string())
        } else {
            Some(source.cell(row, col))
        }
    }

    fn copy_cell(&mut self, _: &CopyCell, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((row, col)) = self.selected_cell {
            if let Some(text) = self.cell_text_for_selection(row, col) {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                let preview = if text.len() > 40 {
                    format!("\"{}...\" copied", &text[..37])
                } else {
                    format!("\"{text}\" copied")
                };
                self.show_toast(preview, window, cx);
            }
        }
    }

    fn select_and_maybe_copy(
        &mut self,
        row: usize,
        col: usize,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_cell = Some((row, col));
        if click_count >= 2 {
            if let Some(text) = self.cell_text_for_selection(row, col) {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                let preview = if text.len() > 40 {
                    format!("\"{}...\" copied", &text[..37])
                } else {
                    format!("\"{text}\" copied")
                };
                self.show_toast(preview, window, cx);
            }
        }
        window.refresh();
    }

    fn scroll_left(&mut self, _: &ScrollLeft, window: &mut Window, _cx: &mut Context<Self>) {
        self.h_offset = (self.h_offset - SCROLL_STEP).max(0.0);
        window.refresh();
    }

    fn scroll_right(&mut self, _: &ScrollRight, window: &mut Window, _cx: &mut Context<Self>) {
        self.h_offset = (self.h_offset + SCROLL_STEP).min(self.h_max_offset);
        window.refresh();
    }

    // -- Scrollbar metrics --

    fn v_scroll_metrics(&self, track_height: f32) -> Option<(f32, f32, f32)> {
        // Use cached v_max_offset (from last full render) as the authoritative max,
        // because during deferred drag the live max_offset may be stale/zero.
        let max_off = if self.v_max_offset > 0.0 {
            self.v_max_offset
        } else {
            let state = self.v_scroll_handle.0.borrow();
            let live: f32 = state.base_handle.max_offset().y.into();
            live
        };
        if max_off <= 0.0 {
            return None;
        }
        let state = self.v_scroll_handle.0.borrow();
        let cur_off: f32 = (-state.base_handle.offset().y).into();
        let ratio = (cur_off / max_off).clamp(0.0, 1.0);
        let content_h = track_height + max_off;
        let thumb_h = (track_height / content_h * track_height).max(SCROLLBAR_MIN_THUMB);
        let thumb_top = ratio * (track_height - thumb_h);
        Some((thumb_h, thumb_top, max_off))
    }

    fn h_scroll_metrics(&self, track_width: f32) -> Option<(f32, f32)> {
        if self.h_max_offset <= 0.0 {
            return None;
        }
        let ratio = self.h_offset / self.h_max_offset;
        let thumb_w = (track_width / self.data_width * track_width).max(SCROLLBAR_MIN_THUMB);
        let thumb_left = ratio * (track_width - thumb_w);
        Some((thumb_w, thumb_left))
    }

    // -- Scrollbar drag handlers --

    fn on_v_track_click(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.v_track_height <= 0.0 || self.v_max_offset <= 0.0 {
            return;
        }
        // Jump: map click Y to scroll offset via set_offset
        let file_header_h = to_f32(window.viewport_size().height) - self.v_track_height - HEADER_HEIGHT;
        let click_in_track = to_f32(event.position.y) - file_header_h - HEADER_HEIGHT;
        let ratio = (click_in_track / self.v_track_height).clamp(0.0, 1.0);
        let new_offset = ratio * self.v_max_offset;
        {
            let state = self.v_scroll_handle.0.borrow();
            state.base_handle.set_offset(point(state.base_handle.offset().x, px(-new_offset)));
        }

        // Start drag
        self.dragging = Some(DragTarget::Vertical);
        self.drag_start_pos = to_f32(event.position.y);
        self.drag_start_offset = new_offset;
        window.refresh();
    }

    fn on_h_track_click(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.h_track_width <= 0.0 {
            return;
        }
        // Jump: map click X to h_offset
        let click_in_track = to_f32(event.position.x) - self.row_num_width;
        let ratio = (click_in_track / self.h_track_width).clamp(0.0, 1.0);
        self.h_offset = (ratio * self.h_max_offset).min(self.h_max_offset);

        // Start drag
        self.dragging = Some(DragTarget::Horizontal);
        self.drag_start_pos = to_f32(event.position.x);
        self.drag_start_offset = self.h_offset;
        window.refresh();
    }

    fn on_global_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.dragging else { return };
        match target {
            DragTarget::Vertical => {
                if self.v_track_height <= 0.0 || self.v_max_offset <= 0.0 {
                    return;
                }
                let delta_px = to_f32(event.position.y) - self.drag_start_pos;
                let scale = self.v_max_offset / self.v_track_height;
                let new_offset = (self.drag_start_offset + delta_px * scale).clamp(0.0, self.v_max_offset);
                {
                    let state = self.v_scroll_handle.0.borrow();
                    state.base_handle.set_offset(point(state.base_handle.offset().x, px(-new_offset)));
                }
                // Movement detected: reset idle, bump generation, schedule idle check
                self.drag_idle = false;
                self.drag_generation = self.drag_generation.wrapping_add(1);
                let generation = self.drag_generation;
                cx.spawn_in(window, async move |_this, cx| {
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                    cx.update(|window, cx| {
                        if let Some(entity) = _this.upgrade() {
                            entity.update(cx, |view, _cx| {
                                if view.dragging == Some(DragTarget::Vertical)
                                    && view.drag_generation == generation
                                {
                                    view.drag_idle = true;
                                }
                            });
                            window.refresh();
                        }
                    }).ok();
                }).detach();
                window.refresh();
            }
            DragTarget::Horizontal => {
                if self.h_track_width <= 0.0 {
                    return;
                }
                let delta_px = to_f32(event.position.x) - self.drag_start_pos;
                let scale = self.h_max_offset / self.h_track_width;
                self.h_offset = (self.drag_start_offset + delta_px * scale).clamp(0.0, self.h_max_offset);
                // Same idle pattern as vertical
                self.drag_idle = false;
                self.drag_generation = self.drag_generation.wrapping_add(1);
                let generation = self.drag_generation;
                cx.spawn_in(window, async move |_this, cx| {
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                    cx.update(|window, cx| {
                        if let Some(entity) = _this.upgrade() {
                            entity.update(cx, |view, _cx| {
                                if view.dragging == Some(DragTarget::Horizontal)
                                    && view.drag_generation == generation
                                {
                                    view.drag_idle = true;
                                }
                            });
                            window.refresh();
                        }
                    }).ok();
                }).detach();
                window.refresh();
            }
        }
    }

    fn on_global_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.dragging.is_some() {
            self.dragging = None;
            self.drag_idle = false;
            window.refresh();
        }
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.state {
            AppState::Error(msg) => {
                let view = cx.entity().downgrade();
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .size_full()
                    .justify_center()
                    .items_center()
                    .child(
                        div()
                            .text_color(rgb(0xf38ba8))
                            .text_xl()
                            .child(msg.clone()),
                    )
                    .child(
                        div()
                            .id("open-file-btn")
                            .px_4()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(0x45475a))
                            .text_color(rgb(0xcdd6f4))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0x585b70)))
                            .child("Open File (Ctrl+O)")
                            .on_click(move |_, window, cx| {
                                if let Some(entity) = view.upgrade() {
                                    entity.update(cx, |this, cx| {
                                        this.open_file(&OpenFile, window, cx);
                                    });
                                }
                            }),
                    )
                    .into_any_element()
            }

            AppState::Loaded { file_name, source } => {
                let col_count = source.col_count();
                let row_count = source.row_count();
                let row_num_w = self.row_num_width;
                let viewport_w: f32 = (window.viewport_size().width - px(row_num_w)).into();
                let (vis_first, vis_last, spacer) = self.visible_col_range(viewport_w);
                let vis_widths: Vec<f32> = self.col_widths[vis_first..vis_last].to_vec();

                // File info header
                let file_header = div()
                    .pb_2()
                    .px_4()
                    .pt_4()
                    .border_b_1()
                    .border_color(rgb(0x45475a))
                    .child(
                        div()
                            .text_xl()
                            .text_color(rgb(0x89b4fa))
                            .child(file_name.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x6c7086))
                            .child(format!("{col_count} columns · {row_count} rows")),
                    );

                // -- Column header row --
                let header_bg = rgb(0x313244);
                let header_selected_bg = rgb(0x585b70);
                let header_view = cx.entity().downgrade();
                let mut header_data = div().flex().ml(px(spacer));
                for (i, col) in (vis_first..vis_last).enumerate() {
                    let is_selected = self.selected_cell == Some((Self::HEADER_ROW, col));
                    let bg = if is_selected { header_selected_bg } else { header_bg };
                    let view = header_view.clone();
                    header_data = header_data.child(
                        div()
                            .id(ElementId::NamedInteger("hdr".into(), col as u64))
                            .w(px(vis_widths[i]))
                            .h(px(HEADER_HEIGHT))
                            .px_2()
                            .py_1()
                            .border_r_1()
                            .border_b_1()
                            .border_color(rgb(0x45475a))
                            .bg(bg)
                            .overflow_hidden()
                            .cursor_pointer()
                            .on_click(move |event: &ClickEvent, window, cx| {
                                if let Some(entity) = view.upgrade() {
                                    let count = event.click_count();
                                    entity.update(cx, |this, cx| {
                                        this.select_and_maybe_copy(RootView::HEADER_ROW, col, count, window, cx);
                                    });
                                }
                            })
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x89b4fa))
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(source.column_name(col).to_string()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .text_xs()
                                    .text_color(rgb(0x6c7086))
                                    .child(format!("#{}", col + 1))
                                    .child(format!("type: {}", source.column_type(col))),
                            ),
                    );
                }

                let header_row = div()
                    .flex()
                    .h(px(HEADER_HEIGHT))
                    .child(
                        div()
                            .w(px(row_num_w))
                            .h(px(HEADER_HEIGHT))
                            .px_2()
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_b_1()
                            .border_color(rgb(0x45475a))
                            .bg(header_bg)
                            .child(div().text_sm().text_color(rgb(0x6c7086)).child("#")),
                    )
                    .child(
                        div()
                            .w(px(viewport_w))
                            .h(px(HEADER_HEIGHT))
                            .overflow_hidden()
                            .child(header_data),
                    );

                // -- Data rows --
                let source_ref = Rc::clone(source);
                let widths = vis_widths;
                let dragging_fast = self.dragging.is_some() && !self.drag_idle;
                let selected = self.selected_cell;
                let view_entity = cx.entity().downgrade();
                let data_rows = uniform_list(
                    "data-rows",
                    row_count,
                    move |range, _window, _cx| {
                        range
                            .map(|row_idx| {
                                let bg = if row_idx % 2 == 0 {
                                    rgb(0x1e1e2e)
                                } else {
                                    rgb(0x181825)
                                };

                                let mut data_cells = div().flex().ml(px(spacer));
                                // During drag: show data if cached (free), empty if not (skip I/O)
                                let show_data = !dragging_fast || source_ref.is_cached(row_idx);
                                for (i, col_idx) in (vis_first..vis_last).enumerate() {
                                    let is_selected = selected == Some((row_idx, col_idx));
                                    let cell_bg = if is_selected {
                                        rgb(0x45475a)
                                    } else {
                                        bg
                                    };
                                    let mut cell = div()
                                        .id(ElementId::NamedInteger("cell".into(), (row_idx * 10000 + col_idx) as u64))
                                        .w(px(widths[i]))
                                        .h(px(ROW_HEIGHT))
                                        .border_r_1()
                                        .border_color(rgb(0x45475a))
                                        .bg(cell_bg)
                                        .cursor_pointer();
                                    if show_data {
                                        let view = view_entity.clone();
                                        cell = cell
                                            .on_click(move |event: &ClickEvent, window, cx| {
                                                if let Some(entity) = view.upgrade() {
                                                    let count = event.click_count();
                                                    entity.update(cx, |this, cx| {
                                                        this.select_and_maybe_copy(row_idx, col_idx, count, window, cx);
                                                    });
                                                }
                                            })
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(rgb(0xcdd6f4))
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .child(
                                                        source_ref.cell(row_idx, col_idx),
                                                    ),
                                            );
                                    }
                                    data_cells = data_cells.child(cell);
                                }

                                div()
                                    .flex()
                                    .h(px(ROW_HEIGHT))
                                    .bg(bg)
                                    .child(
                                        div()
                                            .w(px(row_num_w))
                                            .h(px(ROW_HEIGHT))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .border_r_1()
                                            .border_color(rgb(0x45475a))
                                            .bg(bg)
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x585b70))
                                                    .child(format!("{}", row_idx + 1)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w(px(viewport_w))
                                            .h(px(ROW_HEIGHT))
                                            .overflow_hidden()
                                            .child(data_cells),
                                    )
                            })
                            .collect()
                    },
                )
                .flex_1()
                .track_scroll(&self.v_scroll_handle);

                // -- Compute layout dimensions for scrollbars --
                // Scrollbar track = window height - file header - column header row
                let window_h: f32 = window.viewport_size().height.into();
                let body_h = (window_h - FILE_HEADER_HEIGHT - HEADER_HEIGHT).max(0.0);
                self.v_track_height = body_h;
                self.h_track_width = viewport_w;
                self.h_max_offset = (self.data_width - viewport_w).max(0.0);

                // Cache v_max_offset for drag handler
                {
                    let state = self.v_scroll_handle.0.borrow();
                    let live_max: f32 = state.base_handle.max_offset().y.into();
                    if live_max > 0.0 {
                        self.v_max_offset = live_max;
                    }
                }

                // -- Scrollbars --
                let v_scrollbar = self.v_scroll_metrics(body_h).map(|(thumb_h, thumb_top, _)| {
                    div()
                        .id("v-scrollbar-track")
                        .absolute()
                        .right_0()
                        .top(px(HEADER_HEIGHT))
                        .w(px(SCROLLBAR_SIZE))
                        .h(px(body_h))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(Self::on_v_track_click),
                        )
                        .child(
                            div()
                                .absolute()
                                .top(px(thumb_top))
                                .w(px(SCROLLBAR_SIZE))
                                .h(px(thumb_h))
                                .rounded(px(SCROLLBAR_SIZE / 2.0))
                                .bg(rgb(0x585b70))
                                .cursor_pointer(),
                        )
                });

                let h_scrollbar = self.h_scroll_metrics(viewport_w).map(|(thumb_w, thumb_left)| {
                    div()
                        .id("h-scrollbar-track")
                        .absolute()
                        .bottom_0()
                        .left(px(row_num_w))
                        .w(px(viewport_w))
                        .h(px(SCROLLBAR_SIZE))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(Self::on_h_track_click),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(thumb_left))
                                .w(px(thumb_w))
                                .h(px(SCROLLBAR_SIZE))
                                .rounded(px(SCROLLBAR_SIZE / 2.0))
                                .bg(rgb(0x585b70))
                                .cursor_pointer(),
                        )
                });

                let body = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(header_row)
                    .child(data_rows);

                let mut table_area = div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(body);

                if let Some(bar) = v_scrollbar {
                    table_area = table_area.child(bar);
                }
                if let Some(bar) = h_scrollbar {
                    table_area = table_area.child(bar);
                }

                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(file_header)
                    .child(table_area)
                    .into_any_element()
            }
        };

        let mut root = div()
            .id("root")
            .relative()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::scroll_up))
            .on_action(cx.listener(Self::scroll_down))
            .on_action(cx.listener(Self::scroll_left))
            .on_action(cx.listener(Self::scroll_right))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::copy_cell))
            .on_action(cx.listener(Self::open_file))
            .on_mouse_move(cx.listener(Self::on_global_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_global_mouse_up))
            .size_full()
            .bg(rgb(0x1e1e2e))
            .child(content);

        // Toast notification overlay
        if let Some(msg) = &self.toast {
            root = root.child(
                div()
                    .absolute()
                    .bottom(px(24.0))
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .px_4()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(0x45475a))
                            .text_sm()
                            .text_color(rgb(0xcdd6f4))
                            .child(msg.clone()),
                    ),
            );
        }

        root
    }
}
