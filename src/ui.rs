use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{prelude::*, *};

use crate::data::DataSource;

const ROW_NUM_WIDTH: f32 = 60.0;
const ROW_HEIGHT: f32 = 28.0;
const HEADER_HEIGHT: f32 = 58.0;
const SCROLL_STEP: f32 = 300.0;
const SCROLLBAR_SIZE: f32 = 10.0;
const SCROLLBAR_MIN_THUMB: f32 = 20.0;
/// Inset around the table so rounded window corners don't clip the scrollbars or cells.
const TABLE_MARGIN: f32 = 10.0;

const ROW_STEP: usize = 20;
const PAGE_STEP: usize = 200;

fn to_f32(p: Pixels) -> f32 {
    let v: f32 = p.into();
    v
}

/// Scrollbar geometry along the scroll axis, in window coordinates.
/// Written while painting, read by the mouse handlers.
#[derive(Clone, Copy)]
struct ThumbGeom {
    track_origin: f32,
    track_len: f32,
    thumb_start: f32, // relative to track_origin
    thumb_len: f32,
}

/// `visible` is the fraction of the content shown in the viewport,
/// `position` how far it is scrolled (0 = start, 1 = end).
fn thumb_geom(track_origin: f32, track_len: f32, visible: f32, position: f32) -> Option<ThumbGeom> {
    if track_len <= 0.0 || visible >= 1.0 {
        return None;
    }
    let thumb_len = (track_len * visible).max(SCROLLBAR_MIN_THUMB).min(track_len);
    let thumb_start = position.clamp(0.0, 1.0) * (track_len - thumb_len);
    Some(ThumbGeom {
        track_origin,
        track_len,
        thumb_start,
        thumb_len,
    })
}

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

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragTarget {
    Vertical,
    Horizontal,
}

/// Column geometry derived from a data source.
struct ColumnLayout {
    row_num_width: f32,
    col_widths: Vec<f32>,
    col_positions: Vec<f32>,
    data_width: f32,
}

impl ColumnLayout {
    fn new(source: Option<&dyn DataSource>) -> Self {
        let Some(source) = source else {
            return Self {
                row_num_width: ROW_NUM_WIDTH,
                col_widths: vec![],
                col_positions: vec![],
                data_width: 0.0,
            };
        };
        let col_widths = source.column_widths();
        let mut col_positions = Vec::with_capacity(col_widths.len());
        let mut acc = 0.0f32;
        for &w in &col_widths {
            col_positions.push(acc);
            acc += w;
        }
        let digits = source.row_count().to_string().len();
        Self {
            row_num_width: (digits as f32 * 9.0 + 24.0).max(ROW_NUM_WIDTH),
            col_widths,
            col_positions,
            data_width: acc,
        }
    }

    /// Columns overlapping `[h_offset, h_offset + width)`: first, one past the last,
    /// and the x of the first column's left edge relative to the view (<= 0).
    fn visible_range(&self, h_offset: f32, width: f32) -> (usize, usize, f32) {
        let n = self.col_widths.len();
        if n == 0 {
            return (0, 0, 0.0);
        }
        let right = h_offset + width;
        // Column i ends where column i + 1 starts.
        let first = self.col_positions[1..].partition_point(|&end| end <= h_offset).min(n - 1);
        let last = self.col_positions.partition_point(|&pos| pos < right).min(n);
        (first, last, self.col_positions[first] - h_offset)
    }
}

pub struct RootView {
    state: AppState,
    layout: ColumnLayout,
    focus_handle: FocusHandle,
    // Vertical position in pixels. f64 because f32 (GPUI's Pixels) loses whole
    // pixels past ~16M px, i.e. ~600k rows, and small scroll deltas get dropped.
    v_offset: f64,
    h_offset: f32,
    // Height of the row area, measured at the last paint
    body_height: Rc<Cell<f32>>,
    // Scrollbar drag state
    dragging: Option<DragTarget>,
    drag_start_pos: f32,    // mouse position when drag started
    drag_start_offset: f64, // scroll offset when drag started
    drag_scale: f64,        // scroll pixels per mouse pixel
    drag_idle: bool,        // true when drag paused long enough to show data
    drag_generation: usize, // bumped on each mouse move, idle timer checks this
    selected_cell: Option<(usize, usize)>, // (row, col)
    toast: Option<String>,
    toast_generation: usize,
    // Scrollbar geometry from the last paint
    v_thumb: Rc<Cell<Option<ThumbGeom>>>,
    h_thumb: Rc<Cell<Option<ThumbGeom>>>,
    // Axis lock for trackpad gestures, so a vertical swipe doesn't drift sideways
    scroll_gesture: OngoingScroll,
    // Updated each render
    h_max_offset: f32,
}

impl RootView {
    pub fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let layout = match &state {
            AppState::Loaded { source, .. } => ColumnLayout::new(Some(source.as_ref())),
            _ => ColumnLayout::new(None),
        };
        Self {
            state,
            layout,
            focus_handle: cx.focus_handle(),
            v_offset: 0.0,
            h_offset: 0.0,
            body_height: Rc::new(Cell::new(0.0)),
            dragging: None,
            drag_start_pos: 0.0,
            drag_start_offset: 0.0,
            drag_scale: 1.0,
            drag_idle: false,
            drag_generation: 0,
            selected_cell: None,
            toast: None,
            toast_generation: 0,
            v_thumb: Rc::new(Cell::new(None)),
            h_thumb: Rc::new(Cell::new(None)),
            scroll_gesture: OngoingScroll::default(),
            h_max_offset: 0.0,
        }
    }

    fn row_count(&self) -> usize {
        match &self.state {
            AppState::Loaded { source, .. } => source.row_count(),
            _ => 0,
        }
    }

    // -- Scrolling --

    /// Max vertical offset. Leaves room below the last row for the horizontal
    /// scrollbar so it never covers data.
    fn v_max_offset(&self) -> f64 {
        let bottom_pad = if self.h_max_offset > 0.0 { SCROLLBAR_SIZE } else { 0.0 };
        let content = self.row_count() as f64 * ROW_HEIGHT as f64 + bottom_pad as f64;
        (content - self.body_height.get() as f64).max(0.0)
    }

    fn max_offset(&self, target: DragTarget) -> f64 {
        match target {
            DragTarget::Vertical => self.v_max_offset(),
            DragTarget::Horizontal => self.h_max_offset as f64,
        }
    }

    fn scroll_offset(&self, target: DragTarget) -> f64 {
        match target {
            DragTarget::Vertical => self.v_offset,
            DragTarget::Horizontal => self.h_offset as f64,
        }
    }

    /// Clamps and applies the offset. Returns true when it changed.
    fn set_scroll_offset(&mut self, target: DragTarget, offset: f64) -> bool {
        let offset = offset.clamp(0.0, self.max_offset(target));
        match target {
            DragTarget::Vertical => {
                let changed = offset != self.v_offset;
                self.v_offset = offset;
                changed
            }
            DragTarget::Horizontal => {
                let offset = offset as f32;
                let changed = offset != self.h_offset;
                self.h_offset = offset;
                changed
            }
        }
    }

    fn scroll_by(&mut self, target: DragTarget, delta: f64, cx: &mut Context<Self>) {
        let offset = self.scroll_offset(target) + delta;
        if self.set_scroll_offset(target, offset) {
            cx.notify();
        }
    }

    fn scroll_up(&mut self, _: &ScrollUp, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll_by(DragTarget::Vertical, -((ROW_STEP as f32 * ROW_HEIGHT) as f64), cx);
    }

    fn scroll_down(&mut self, _: &ScrollDown, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll_by(DragTarget::Vertical, (ROW_STEP as f32 * ROW_HEIGHT) as f64, cx);
    }

    fn page_up(&mut self, _: &PageUp, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll_by(DragTarget::Vertical, -((PAGE_STEP as f32 * ROW_HEIGHT) as f64), cx);
    }

    fn page_down(&mut self, _: &PageDown, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll_by(DragTarget::Vertical, (PAGE_STEP as f32 * ROW_HEIGHT) as f64, cx);
    }

    fn scroll_left(&mut self, _: &ScrollLeft, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll_by(DragTarget::Horizontal, -SCROLL_STEP as f64, cx);
    }

    fn scroll_right(&mut self, _: &ScrollRight, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll_by(DragTarget::Horizontal, SCROLL_STEP as f64, cx);
    }

    /// Wheel / trackpad scrolling on both axes. Shift+wheel arrives as an X delta
    /// on every platform.
    fn on_table_scroll(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let mut delta = event.delta.pixel_delta(window.line_height());
        if event.delta.precise() {
            self.scroll_gesture.filter(&mut delta, event.touch_phase);
        }
        // Positive delta moves content right/down, i.e. scrolls towards the start.
        let dx = -to_f32(delta.x) as f64;
        let dy = -to_f32(delta.y) as f64;
        let mut changed = false;
        if dx != 0.0 {
            changed |= self.set_scroll_offset(DragTarget::Horizontal, self.h_offset as f64 + dx);
        }
        if dy != 0.0 {
            changed |= self.set_scroll_offset(DragTarget::Vertical, self.v_offset + dy);
        }
        if changed {
            cx.notify();
        }
    }

    // -- Toast, files, clipboard --

    const HEADER_ROW: usize = usize::MAX;
    const TOAST_DURATION_MS: u64 = 1500;

    fn show_toast(&mut self, msg: String, window: &mut Window, cx: &mut Context<Self>) {
        self.toast = Some(msg);
        self.toast_generation = self.toast_generation.wrapping_add(1);
        let generation = self.toast_generation;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(RootView::TOAST_DURATION_MS))
                .await;
            this.update(cx, |view, cx| {
                if view.toast_generation == generation {
                    view.toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open Parquet file".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update_in(cx, |view, window, cx| view.load_file(&path, window, cx))
                    .ok();
            }
        })
        .detach();
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
                self.layout = ColumnLayout::new(Some(source.as_ref()));
                self.state = AppState::Loaded { file_name, source };
                self.v_offset = 0.0;
                self.h_offset = 0.0;
                self.selected_cell = None;
                self.show_toast(format!("Opened: {}", path.display()), window, cx);
            }
            Err(e) => {
                self.state = AppState::Error(e);
                cx.notify();
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
            Some(source.cell_full(row, col))
        }
    }

    fn copy_to_clipboard(&mut self, row: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.cell_text_for_selection(row, col) else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        let preview = if text.chars().count() > 40 {
            format!("\"{}...\" copied", text.chars().take(37).collect::<String>())
        } else {
            format!("\"{text}\" copied")
        };
        self.show_toast(preview, window, cx);
    }

    fn copy_cell(&mut self, _: &CopyCell, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((row, col)) = self.selected_cell {
            self.copy_to_clipboard(row, col, window, cx);
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
            self.copy_to_clipboard(row, col, window, cx);
        }
        cx.notify();
    }

    // -- Scrollbar drag handlers --

    fn on_scrollbar_mouse_down(
        &mut self,
        target: DragTarget,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let geom = match target {
            DragTarget::Vertical => self.v_thumb.get(),
            DragTarget::Horizontal => self.h_thumb.get(),
        };
        let Some(g) = geom else { return };
        let max = self.max_offset(target);
        let travel = g.track_len - g.thumb_len;
        if max <= 0.0 || travel <= 0.0 {
            return;
        }
        let pos = match target {
            DragTarget::Vertical => to_f32(event.position.y),
            DragTarget::Horizontal => to_f32(event.position.x),
        };
        let in_track = pos - g.track_origin;
        // Clicking the track (not the thumb) centers the thumb on the click,
        // then the drag continues from there.
        if in_track < g.thumb_start || in_track > g.thumb_start + g.thumb_len {
            let ratio = ((in_track - g.thumb_len / 2.0) / travel).clamp(0.0, 1.0);
            self.set_scroll_offset(target, ratio as f64 * max);
        }
        self.dragging = Some(target);
        self.drag_start_pos = pos;
        self.drag_start_offset = self.scroll_offset(target);
        self.drag_scale = max / travel as f64; // thumb follows the cursor 1:1
        self.drag_idle = false;
        cx.notify();
    }

    /// Called from a window-level listener, so it keeps firing when the cursor
    /// leaves the window during a drag.
    fn on_drag_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.dragging else { return };
        // Button no longer held: the release happened somewhere we didn't see
        // (e.g. focus moved to another app). End the drag instead of sticking.
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_drag(cx);
            return;
        }
        let pos = match target {
            DragTarget::Vertical => to_f32(event.position.y),
            DragTarget::Horizontal => to_f32(event.position.x),
        };
        let offset = self.drag_start_offset + (pos - self.drag_start_pos) as f64 * self.drag_scale;
        self.set_scroll_offset(target, offset);

        // Movement detected: reset idle, bump generation, schedule idle check
        self.drag_idle = false;
        self.drag_generation = self.drag_generation.wrapping_add(1);
        let generation = self.drag_generation;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            this.update(cx, |view, cx| {
                if view.dragging.is_some() && view.drag_generation == generation {
                    view.drag_idle = true;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.dragging.is_some() {
            self.dragging = None;
            self.drag_idle = false;
            cx.notify();
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
                let source = Rc::clone(source);
                let file_name = file_name.clone();
                let col_count = source.col_count();
                let row_count = source.row_count();
                let row_num_w = self.layout.row_num_width;

                // -- Horizontal geometry --
                let window_w = to_f32(window.viewport_size().width);
                let viewport_w = (window_w - 2.0 * TABLE_MARGIN - row_num_w).max(0.0);
                // Extra SCROLLBAR_SIZE so the last column can scroll clear of the vertical scrollbar.
                self.h_max_offset = (self.layout.data_width + SCROLLBAR_SIZE - viewport_w).max(0.0);
                self.h_offset = self.h_offset.min(self.h_max_offset); // window may have grown
                let h_scrollable = self.h_max_offset > 0.0;
                let (vis_first, vis_last, spacer) = self.layout.visible_range(self.h_offset, viewport_w);
                let vis_widths: Vec<f32> = self.layout.col_widths[vis_first..vis_last].to_vec();

                // -- Vertical geometry --
                // Rows are virtualized by hand instead of with uniform_list, whose
                // f32 offset breaks down on files with millions of rows.
                let v_max = self.v_max_offset();
                self.v_offset = self.v_offset.min(v_max); // window may have grown
                let row_h = ROW_HEIGHT as f64;
                let first_row = ((self.v_offset / row_h) as usize).min(row_count);
                let row_shift = (self.v_offset - first_row as f64 * row_h) as f32;
                // The window height always covers the row area, even before it's measured.
                let rows_to_draw =
                    (to_f32(window.viewport_size().height) / ROW_HEIGHT).ceil() as usize + 2;
                let last_row = (first_row + rows_to_draw).min(row_count);

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
                            .child(file_name),
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
                let view_entity = cx.entity().downgrade();
                let mut header_data = div().flex().ml(px(spacer));
                for (i, col) in (vis_first..vis_last).enumerate() {
                    let is_selected = self.selected_cell == Some((Self::HEADER_ROW, col));
                    let bg = if is_selected { header_selected_bg } else { header_bg };
                    let view = view_entity.clone();
                    header_data = header_data.child(
                        div()
                            .id(ElementId::NamedInteger("hdr".into(), col as u64))
                            .debug_selector(|| format!("hdr-{col}"))
                            .flex_shrink_0()
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
                                let count = event.click_count();
                                view.update(cx, |this, cx| {
                                    this.select_and_maybe_copy(RootView::HEADER_ROW, col, count, window, cx);
                                })
                                .ok();
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
                    .flex_shrink_0()
                    .h(px(HEADER_HEIGHT))
                    .child(
                        div()
                            .flex_shrink_0()
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
                            .flex_shrink_0()
                            .w(px(viewport_w))
                            .h(px(HEADER_HEIGHT))
                            .overflow_hidden()
                            .child(header_data),
                    );

                // -- Data rows --
                let dragging_fast = self.dragging.is_some() && !self.drag_idle;
                let mut rows = div().absolute().top(px(-row_shift)).left_0().flex().flex_col();
                for row_idx in first_row..last_row {
                    let bg = if row_idx % 2 == 0 {
                        rgb(0x1e1e2e)
                    } else {
                        rgb(0x181825)
                    };

                    let mut data_cells = div().flex().ml(px(spacer));
                    for (i, col_idx) in (vis_first..vis_last).enumerate() {
                        // During drag: show data if cached (free), empty if not (skip I/O)
                        let show_data = !dragging_fast || source.is_cached(row_idx, col_idx);
                        let is_selected = self.selected_cell == Some((row_idx, col_idx));
                        let cell_bg = if is_selected { rgb(0x45475a) } else { bg };
                        let mut cell = div()
                            // Scoped under the row's id, so no collisions however many columns.
                            .id(col_idx)
                            .debug_selector(|| format!("cell-{row_idx}-{col_idx}"))
                            // Fixed width: without this, flex shrinks the cells to fit
                            // the clipped row, and how much depends on h_offset.
                            .flex_shrink_0()
                            .w(px(vis_widths[i]))
                            .h(px(ROW_HEIGHT))
                            .border_r_1()
                            .border_color(rgb(0x45475a))
                            .bg(cell_bg)
                            .cursor_pointer();
                        if show_data {
                            let view = view_entity.clone();
                            cell = cell
                                .on_click(move |event: &ClickEvent, window, cx| {
                                    let count = event.click_count();
                                    view.update(cx, |this, cx| {
                                        this.select_and_maybe_copy(row_idx, col_idx, count, window, cx);
                                    })
                                    .ok();
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
                                        .child(source.cell(row_idx, col_idx)),
                                );
                        }
                        data_cells = data_cells.child(cell);
                    }

                    rows = rows.child(
                        div()
                            .id(row_idx)
                            .flex()
                            .h(px(ROW_HEIGHT))
                            .bg(bg)
                            .child(
                                div()
                                    .flex_shrink_0()
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
                                            .child((row_idx + 1).to_string()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(px(viewport_w))
                                    .h(px(ROW_HEIGHT))
                                    .overflow_hidden()
                                    .child(data_cells),
                            ),
                    );
                }

                // Measure the row area; re-render if it changed (first frame, resize).
                let body_height = self.body_height.clone();
                let measure_view = view_entity.clone();
                let measure = canvas(
                    move |bounds, _, cx| {
                        let h = to_f32(bounds.size.height);
                        if body_height.get() != h {
                            body_height.set(h);
                            cx.defer(move |cx| {
                                measure_view.update(cx, |_, cx| cx.notify()).ok();
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full();

                let data_area = div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(rows)
                    .child(measure);

                // -- Scrollbars --
                // Tracks are sized by layout; the thumb is painted from the track's
                // actual bounds, which are also stored for the mouse handlers.
                let thumb_color = |active: bool| if active { rgb(0x7f849c) } else { rgb(0x585b70) };

                let body_h = self.body_height.get() as f64;
                let v_visible = if v_max > 0.0 { (body_h / (body_h + v_max)) as f32 } else { 1.0 };
                let v_position = if v_max > 0.0 { (self.v_offset / v_max) as f32 } else { 0.0 };
                let v_thumb = self.v_thumb.clone();
                let v_color = thumb_color(self.dragging == Some(DragTarget::Vertical));
                let v_scrollbar = div()
                    .id("v-scrollbar-track")
                    .absolute()
                    .right_0()
                    .top(px(HEADER_HEIGHT))
                    .bottom(px(if h_scrollable { SCROLLBAR_SIZE } else { 0.0 }))
                    .w(px(SCROLLBAR_SIZE))
                    .block_mouse_except_scroll() // don't click the cells underneath
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e, w, cx| {
                            this.on_scrollbar_mouse_down(DragTarget::Vertical, e, w, cx)
                        }),
                    )
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let geom = thumb_geom(
                                    to_f32(bounds.origin.y),
                                    to_f32(bounds.size.height),
                                    v_visible,
                                    v_position,
                                );
                                v_thumb.set(geom);
                                if let Some(g) = geom {
                                    let thumb = Bounds::new(
                                        point(bounds.origin.x, px(g.track_origin + g.thumb_start)),
                                        size(bounds.size.width, px(g.thumb_len)),
                                    );
                                    window.paint_quad(
                                        fill(thumb, v_color).corner_radii(px(SCROLLBAR_SIZE / 2.0)),
                                    );
                                }
                            },
                        )
                        .size_full(),
                    );

                let h_scrollbar = h_scrollable.then(|| {
                    let h_thumb = self.h_thumb.clone();
                    let data_w = self.layout.data_width + SCROLLBAR_SIZE;
                    let h_visible = viewport_w / data_w;
                    let h_position = self.h_offset / self.h_max_offset;
                    let h_color = thumb_color(self.dragging == Some(DragTarget::Horizontal));
                    div()
                        .id("h-scrollbar-track")
                        .absolute()
                        .bottom_0()
                        .left(px(row_num_w))
                        .w(px((viewport_w - SCROLLBAR_SIZE).max(0.0)))
                        .h(px(SCROLLBAR_SIZE))
                        .block_mouse_except_scroll()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, e, w, cx| {
                                this.on_scrollbar_mouse_down(DragTarget::Horizontal, e, w, cx)
                            }),
                        )
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    let geom = thumb_geom(
                                        to_f32(bounds.origin.x),
                                        to_f32(bounds.size.width),
                                        h_visible,
                                        h_position,
                                    );
                                    h_thumb.set(geom);
                                    if let Some(g) = geom {
                                        let thumb = Bounds::new(
                                            point(px(g.track_origin + g.thumb_start), bounds.origin.y),
                                            size(px(g.thumb_len), bounds.size.height),
                                        );
                                        window.paint_quad(
                                            fill(thumb, h_color)
                                                .corner_radii(px(SCROLLBAR_SIZE / 2.0)),
                                        );
                                    }
                                },
                            )
                            .size_full(),
                        )
                });
                if !h_scrollable {
                    self.h_thumb.set(None);
                }

                let body = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(header_row)
                    .child(data_area);

                let mut table_area = div()
                    .id("table-area")
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .mx(px(TABLE_MARGIN))
                    .mb(px(TABLE_MARGIN))
                    .on_scroll(cx.listener(Self::on_table_scroll))
                    .child(body)
                    .child(v_scrollbar);

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
            .size_full()
            .bg(rgb(0x1e1e2e))
            .child(content);

        // While dragging a scrollbar, listen at window level instead of on an element:
        // element handlers only fire while the cursor is over them, so a release outside
        // the window was missed and the drag got stuck. The OS keeps delivering move/up
        // events to the window during a drag (mouse capture on Windows, implicit grab on
        // X11/Wayland, Cocoa on macOS).
        if self.dragging.is_some() {
            let view = cx.entity().downgrade();
            root = root.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let move_view = view.clone();
                        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Bubble {
                                move_view.update(cx, |this, cx| this.on_drag_move(e, window, cx)).ok();
                            }
                        });
                        window.on_mouse_event(move |e: &MouseUpEvent, phase, _window, cx| {
                            if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                                view.update(cx, |this, cx| this.end_drag(cx)).ok();
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            );
        }

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

#[cfg(test)]
mod tests {
    use super::{
        AppState, ColumnLayout, DataSource, DragTarget, HEADER_HEIGHT, ROW_HEIGHT, RootView, SCROLLBAR_MIN_THUMB,
        SCROLLBAR_SIZE, TABLE_MARGIN, register_actions, thumb_geom,
    };
    use gpui::{
        Entity, Focusable, Modifiers, MouseButton, MouseMoveEvent, Pixels, Point, ScrollDelta, ScrollEvent,
        TestAppContext, TouchPhase, VisualTestContext, point, px, size,
    };
    use std::rc::Rc;
    use std::time::Duration;

    // -- Pure geometry --

    #[test]
    fn thumb_geom_sizes_and_places_the_thumb() {
        // Everything visible, or no track: no thumb.
        assert!(thumb_geom(0.0, 100.0, 1.0, 0.0).is_none());
        assert!(thumb_geom(0.0, 0.0, 0.5, 0.0).is_none());

        let g = thumb_geom(10.0, 200.0, 0.25, 0.0).unwrap();
        assert_eq!((g.track_origin, g.track_len, g.thumb_len, g.thumb_start), (10.0, 200.0, 50.0, 0.0));
        let g = thumb_geom(10.0, 200.0, 0.25, 1.0).unwrap();
        assert_eq!(g.thumb_start, 150.0); // flush with the track end
        let g = thumb_geom(10.0, 200.0, 0.25, 0.5).unwrap();
        assert_eq!(g.thumb_start, 75.0);
        // Out-of-range positions are clamped.
        assert_eq!(thumb_geom(0.0, 200.0, 0.25, 2.0).unwrap().thumb_start, 150.0);
        assert_eq!(thumb_geom(0.0, 200.0, 0.25, -1.0).unwrap().thumb_start, 0.0);
        // Huge files: the thumb keeps a minimum size, and still ends flush.
        let g = thumb_geom(0.0, 200.0, 1e-7, 1.0).unwrap();
        assert_eq!(g.thumb_len, SCROLLBAR_MIN_THUMB);
        assert_eq!(g.thumb_start + g.thumb_len, 200.0);
    }

    #[test]
    fn column_layout_accumulates_positions() {
        let source = FakeSource::new(5, &[100.0, 50.0, 200.0]);
        let l = ColumnLayout::new(Some(&source));
        assert_eq!(l.col_positions, vec![0.0, 100.0, 150.0]);
        assert_eq!(l.data_width, 350.0);

        // The row number column grows with the digits of the row count.
        let small = ColumnLayout::new(Some(&FakeSource::new(5, &[100.0])));
        let huge = ColumnLayout::new(Some(&FakeSource::new(10_000_000_000, &[100.0])));
        assert!(huge.row_num_width > small.row_num_width);

        let empty = ColumnLayout::new(None);
        assert!(empty.col_widths.is_empty());
        assert_eq!(empty.visible_range(0.0, 500.0), (0, 0, 0.0));
    }

    #[test]
    fn visible_range_covers_partly_visible_columns() {
        let l = ColumnLayout::new(Some(&FakeSource::new(5, &[100.0, 50.0, 200.0, 100.0])));
        // Columns start at 0, 100, 150, 350; data ends at 450.
        assert_eq!(l.visible_range(0.0, 120.0), (0, 2, 0.0));
        assert_eq!(l.visible_range(0.0, 100.0), (0, 1, 0.0));
        // Scrolled into the middle of column 0.
        assert_eq!(l.visible_range(40.0, 100.0), (0, 2, -40.0));
        // Exactly at a column edge: the previous column is gone.
        assert_eq!(l.visible_range(100.0, 60.0), (1, 3, 0.0));
        // Inside one wide column.
        assert_eq!(l.visible_range(200.0, 50.0), (2, 3, -50.0));
        // At the end.
        assert_eq!(l.visible_range(400.0, 100.0), (3, 4, -50.0));
    }

    // -- View harness --

    struct FakeSource {
        rows: usize,
        widths: Vec<f32>,
    }

    impl FakeSource {
        fn new(rows: usize, widths: &[f32]) -> Self {
            Self { rows, widths: widths.to_vec() }
        }
    }

    impl DataSource for FakeSource {
        fn row_count(&self) -> usize {
            self.rows
        }
        fn col_count(&self) -> usize {
            self.widths.len()
        }
        fn column_name(&self, _col: usize) -> &str {
            "col"
        }
        fn column_type(&self, _col: usize) -> &str {
            "Utf8"
        }
        fn cell(&self, row: usize, col: usize) -> String {
            format!("{row}:{col}")
        }
        fn cell_full(&self, row: usize, col: usize) -> String {
            format!("{row}:{col} full")
        }
        fn column_widths(&self) -> Vec<f32> {
            self.widths.clone()
        }
        fn is_cached(&self, _row: usize, _col: usize) -> bool {
            true
        }
    }

    const WINDOW_W: f32 = 800.0;
    const WINDOW_H: f32 = 600.0;

    fn open_view<'a>(
        cx: &'a mut TestAppContext,
        rows: usize,
        widths: &[f32],
    ) -> (Entity<RootView>, &'a mut VisualTestContext) {
        let source = Rc::new(FakeSource::new(rows, widths));
        cx.update(register_actions);
        let (view, cx) = cx.add_window_view(|_, cx| {
            RootView::new(AppState::Loaded { file_name: "test.parquet".into(), source }, cx)
        });
        cx.simulate_resize(size(px(WINDOW_W), px(WINDOW_H)));
        cx.update(|window, cx| view.focus_handle(cx).focus(window, cx));
        cx.run_until_parked();
        (view, cx)
    }

    /// Width of the data columns area, right of the row numbers.
    fn viewport_w(view: &Entity<RootView>, cx: &mut VisualTestContext) -> f32 {
        WINDOW_W - 2.0 * TABLE_MARGIN - read(view, cx, |v| v.layout.row_num_width)
    }

    /// Ten columns of 150 px: wider than the window, so both axes scroll.
    const WIDE: &[f32] = &[150.0; 10];

    fn read<R>(view: &Entity<RootView>, cx: &mut VisualTestContext, f: impl FnOnce(&RootView) -> R) -> R {
        view.read_with(cx, |v, _| f(v))
    }

    fn wheel(cx: &mut VisualTestContext, dx: f32, dy: f32) {
        cx.simulate_event(ScrollEvent {
            position: point(px(WINDOW_W / 2.0), px(WINDOW_H / 2.0)),
            delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
            touch_phase: TouchPhase::Started,
            ..Default::default()
        });
        cx.run_until_parked();
    }

    fn mouse_move(cx: &mut VisualTestContext, pos: Point<Pixels>, button: Option<MouseButton>) {
        cx.simulate_event(MouseMoveEvent { position: pos, pressed_button: button, modifiers: Modifiers::none() });
        cx.run_until_parked();
    }

    /// Centre of the vertical thumb, in window coordinates.
    fn v_thumb_center(view: &Entity<RootView>, cx: &mut VisualTestContext) -> Point<Pixels> {
        let g = read(view, cx, |v| v.v_thumb.get()).expect("vertical thumb");
        point(
            px(WINDOW_W - TABLE_MARGIN - SCROLLBAR_SIZE / 2.0),
            px(g.track_origin + g.thumb_start + g.thumb_len / 2.0),
        )
    }

    #[gpui::test]
    fn view_measures_its_geometry(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let (body, h_max) = read(&view, cx, |v| (v.body_height.get(), v.h_max_offset));
        assert!(body > 0.0 && body < WINDOW_H - HEADER_HEIGHT, "body height {body}");
        let viewport_w = viewport_w(&view, cx);
        assert_eq!(h_max, 1500.0 + SCROLLBAR_SIZE - viewport_w);
        assert!(read(&view, cx, |v| v.v_thumb.get().is_some() && v.h_thumb.get().is_some()));
    }

    // -- Wheel and trackpad --

    #[gpui::test]
    fn wheel_scrolls_and_clamps_both_axes(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        wheel(cx, 0.0, -100.0);
        assert_eq!(read(&view, cx, |v| v.v_offset), 100.0);
        wheel(cx, -70.0, 0.0);
        assert_eq!(read(&view, cx, |v| v.h_offset), 70.0);

        // Past the ends: clamped.
        wheel(cx, 0.0, -1e7);
        wheel(cx, -1e7, 0.0);
        let (v_off, v_max, h_off, h_max) =
            read(&view, cx, |v| (v.v_offset, v.v_max_offset(), v.h_offset, v.h_max_offset));
        assert_eq!((v_off, h_off), (v_max, h_max));
        // At the bottom, the last row sits right above the horizontal scrollbar.
        let visible = read(&view, cx, |v| v.body_height.get() - SCROLLBAR_SIZE) as f64;
        assert_eq!(1000.0 * ROW_HEIGHT as f64 - v_off, visible);
        // At the right, the last column ends right before the vertical scrollbar.
        let cols_w = viewport_w(&view, cx) - SCROLLBAR_SIZE;
        assert_eq!(1500.0 - h_off, cols_w);

        wheel(cx, 1e7, 0.0);
        wheel(cx, 0.0, 1e7);
        assert_eq!(read(&view, cx, |v| (v.v_offset, v.h_offset)), (0.0, 0.0));
    }

    #[gpui::test]
    fn trackpad_gesture_locks_to_its_main_axis(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        // Mostly vertical swipe with some sideways drift.
        wheel(cx, -3.0, -40.0);
        assert_eq!(read(&view, cx, |v| (v.v_offset, v.h_offset)), (40.0, 0.0));
    }

    #[gpui::test]
    fn small_scrolls_still_move_deep_in_huge_files(cx: &mut TestAppContext) {
        // 10M rows is ~280M px, past where f32 can hold whole pixels.
        let (view, cx) = open_view(cx, 10_000_000, WIDE);
        wheel(cx, 0.0, -1e10);
        let bottom = read(&view, cx, |v| v.v_offset);
        assert!(bottom > 2.0e8);
        wheel(cx, 0.0, 1.0);
        assert_eq!(read(&view, cx, |v| v.v_offset), bottom - 1.0);
    }

    #[gpui::test]
    fn growing_the_window_clamps_offsets(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 100, WIDE);
        wheel(cx, -1e7, 0.0);
        wheel(cx, 0.0, -1e7);
        assert!(read(&view, cx, |v| v.h_offset > 0.0 && v.v_offset > 0.0));
        cx.simulate_resize(size(px(2400.0), px(1800.0)));
        cx.run_until_parked();
        let (v_off, v_max, h_off, h_max) =
            read(&view, cx, |v| (v.v_offset, v.v_max_offset(), v.h_offset, v.h_max_offset));
        assert_eq!(h_max, 0.0); // all columns fit now
        assert_eq!(h_off, 0.0);
        assert!(v_off <= v_max);
    }

    #[gpui::test]
    fn columns_keep_their_width_while_scrolling_sideways(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 100, WIDE);
        let left = TABLE_MARGIN + read(&view, cx, |v| v.layout.row_num_width);
        for offset in [0.0, 75.0, 300.0, 333.0] {
            let current = read(&view, cx, |v| v.h_offset);
            wheel(cx, current - offset, 0.0);
            assert_eq!(read(&view, cx, |v| v.h_offset), offset);
            let width = viewport_w(&view, cx);
            let (first, last, _) = read(&view, cx, |v| v.layout.visible_range(v.h_offset, width));
            for col in first..last {
                for key in [format!("hdr-{col}"), format!("cell-0-{col}")] {
                    let key: &'static str = Box::leak(key.into_boxed_str());
                    let b = cx.debug_bounds(key).unwrap_or_else(|| panic!("{key} not drawn at offset {offset}"));
                    assert_eq!(b.size.width, px(150.0), "{key} at offset {offset}");
                    assert_eq!(b.origin.x, px(left + 150.0 * col as f32 - offset), "{key} at offset {offset}");
                }
            }
        }
    }

    // -- Scrollbar dragging --

    #[gpui::test]
    fn dragging_the_thumb_follows_the_cursor(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let start = v_thumb_center(&view, cx);
        let g0 = read(&view, cx, |v| v.v_thumb.get()).unwrap();

        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        // Grabbing the thumb doesn't move it.
        assert_eq!(read(&view, cx, |v| v.v_offset), 0.0);
        assert_eq!(read(&view, cx, |v| v.dragging), Some(DragTarget::Vertical));

        mouse_move(cx, start + point(px(0.0), px(50.0)), Some(MouseButton::Left));
        let g1 = read(&view, cx, |v| v.v_thumb.get()).unwrap();
        assert!((g1.thumb_start - g0.thumb_start - 50.0).abs() < 0.01, "thumb moved {}", g1.thumb_start);
        let (off, max) = read(&view, cx, |v| (v.v_offset, v.v_max_offset()));
        let travel = (g0.track_len - g0.thumb_len) as f64;
        assert!((off - 50.0 * max / travel).abs() < 0.01);

        // A click on the scrollbar doesn't select the cell underneath.
        cx.simulate_mouse_up(start, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(read(&view, cx, |v| v.selected_cell), None);
    }

    #[gpui::test]
    fn horizontal_thumb_drags_too(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let g = read(&view, cx, |v| v.h_thumb.get()).unwrap();
        let start = point(
            px(g.track_origin + g.thumb_start + g.thumb_len / 2.0),
            px(WINDOW_H - TABLE_MARGIN - SCROLLBAR_SIZE / 2.0),
        );
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        mouse_move(cx, start + point(px(1e5), px(0.0)), Some(MouseButton::Left));
        let (off, max) = read(&view, cx, |v| (v.h_offset, v.h_max_offset));
        assert_eq!(off, max);
    }

    #[gpui::test]
    fn release_outside_the_window_ends_the_drag(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let start = v_thumb_center(&view, cx);
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        mouse_move(cx, point(px(-200.0), px(-200.0)), Some(MouseButton::Left));
        cx.simulate_mouse_up(point(px(-200.0), px(-200.0)), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(read(&view, cx, |v| v.dragging), None);

        // Moving afterwards doesn't scroll.
        let off = read(&view, cx, |v| v.v_offset);
        mouse_move(cx, start + point(px(0.0), px(100.0)), None);
        assert_eq!(read(&view, cx, |v| v.v_offset), off);
    }

    #[gpui::test]
    fn missed_release_ends_the_drag_on_next_move(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let start = v_thumb_center(&view, cx);
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        // The release happened in another app; the next move has no button held.
        mouse_move(cx, start + point(px(0.0), px(80.0)), None);
        assert_eq!(read(&view, cx, |v| v.dragging), None);
        assert_eq!(read(&view, cx, |v| v.v_offset), 0.0);
    }

    #[gpui::test]
    fn clicking_the_track_centres_the_thumb_there(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let g = read(&view, cx, |v| v.v_thumb.get()).unwrap();
        let x = px(WINDOW_W - TABLE_MARGIN - SCROLLBAR_SIZE / 2.0);
        // Middle of the track.
        cx.simulate_mouse_down(point(x, px(g.track_origin + g.track_len / 2.0)), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(point(x, px(g.track_origin + g.track_len / 2.0)), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        let (off, max) = read(&view, cx, |v| (v.v_offset, v.v_max_offset()));
        assert!((off / max - 0.5).abs() < 0.001, "at {}", off / max);
        // Very end of the track.
        cx.simulate_mouse_down(point(x, px(g.track_origin + g.track_len - 1.0)), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(read(&view, cx, |v| v.v_offset), max);
    }

    #[gpui::test]
    fn drag_shows_data_after_a_pause(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let start = v_thumb_center(&view, cx);
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        mouse_move(cx, start + point(px(0.0), px(30.0)), Some(MouseButton::Left));
        assert!(!read(&view, cx, |v| v.drag_idle));
        cx.executor().advance_clock(Duration::from_millis(150));
        cx.run_until_parked();
        assert!(read(&view, cx, |v| v.drag_idle));
        // Moving again resets it.
        mouse_move(cx, start + point(px(0.0), px(40.0)), Some(MouseButton::Left));
        assert!(!read(&view, cx, |v| v.drag_idle));
    }
}
