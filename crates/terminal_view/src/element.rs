//! 端末のグリッドを塗る custom Element。
//!
//! 塗る順（後ほど上）: 面 → 既定でない背景 → 検索の一致 → 選択 → カーソル → 文字 → 装飾
//! （下線・取り消し線・リンクの下線）。**選択は背景の後**: 先に敷くと色付きの背景
//! （`ls --color`・TUI の帯）の上で選択が見えなかった（2026-09-26・実画面で確認）。
//!
//! 下線は SGR 4 の全種（単線・二重 4:2・波線 4:3・点線 4:4・破線 4:5）と下線色（SGR 58）。
//! GPUI の `UnderlineStyle` は単線と波線しか持たないので、二重・点線・破線は細い矩形で描く。
//! 点線・破線は行の原点に合わせた周期で描く（セルごとに描いても隣と繋がって見える）。

use alacritty_terminal::index::Point as AlacPoint;
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::Match;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
    fill, point, px, size, App, Bounds, CursorStyle, DispatchPhase, Element, ElementId,
    ElementInputHandler, Entity, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    IntoElement, LayoutId, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, SharedString,
    StrikethroughStyle, Style, TextRun, UnderlineStyle, Window,
};
use theme_core::Theme;

use crate::{
    ansi_to_hsla, background_hsla, is_default_background, term_probe, GridFrame, RenderCell,
    TerminalColors, TerminalLink, TerminalView,
};

/// 薄い（DIM）文字の不透明度。
const DIM_OPACITY: f32 = 0.66;

pub(crate) struct TerminalElement {
    pub(crate) terminal: Entity<TerminalView>,
}

pub(crate) struct TerminalPrepaint {
    cells: Vec<RenderCell>,
    cursor: Option<AlacPoint>,
    cursor_shape: CursorShape,
    /// マウス選択の範囲（グリッド座標）。
    selection: Option<SelectionRange>,
    /// スクロールバックの表示オフセット。グリッド座標 → 表示行は `line + display_offset`。
    display_offset: usize,
    frame: GridFrame,
    /// 行の上端から下線・取り消し線までの距離（フォントの寸法から決める）。
    underline_offset: Pixels,
    strikethrough_offset: Pixels,
    focused: bool,
    theme: Theme,
    /// 取り込んだ配色（O25・無ければ既定）。
    colors: TerminalColors,
    links: Vec<TerminalLink>,
    hovered_link: Option<usize>,
    /// 検索の一致（表示範囲）と、いま見ている一致。
    search_matches: Vec<Match>,
    current_match: Option<Match>,
    columns: usize,
    /// 文字の大きさ（見た目の設定・O25）。セルごとの shape に使う。
    font_size: Pixels,
    hitbox: Hitbox,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

/// 下線の種類（SGR 4 の副引数・21）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum UnderlineKind {
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

fn underline_kind(flags: Flags) -> Option<UnderlineKind> {
    if flags.contains(Flags::DOUBLE_UNDERLINE) {
        Some(UnderlineKind::Double)
    } else if flags.contains(Flags::UNDERCURL) {
        Some(UnderlineKind::Curly)
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        Some(UnderlineKind::Dotted)
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        Some(UnderlineKind::Dashed)
    } else if flags.contains(Flags::UNDERLINE) {
        Some(UnderlineKind::Single)
    } else {
        None
    }
}

/// 点線・破線: 行の原点から `period` ごとに `on` の長さだけ、このセルの幅の分を描く。
fn paint_patterned_line(
    window: &mut Window,
    row_origin_x: Pixels,
    x: Pixels,
    y: Pixels,
    width: Pixels,
    on: f32,
    period: f32,
    color: Hsla,
) {
    let start = f32::from(x - row_origin_x);
    let end = start + f32::from(width);
    let mut step = (start / period).floor();
    while step * period < end {
        let segment_start = (step * period).max(start);
        let segment_end = (step * period + on).min(end);
        if segment_end > segment_start {
            window.paint_quad(fill(
                Bounds::new(
                    point(row_origin_x + px(segment_start), y),
                    size(px(segment_end - segment_start), px(1.)),
                ),
                color,
            ));
        }
        step += 1.0;
    }
}

/// 1 セル分の下線。
fn paint_underline_kind(
    window: &mut Window,
    kind: UnderlineKind,
    row_origin_x: Pixels,
    origin: gpui::Point<Pixels>,
    width: Pixels,
    color: Hsla,
) {
    let solid = UnderlineStyle {
        thickness: px(1.),
        color: Some(color),
        wavy: false,
    };
    match kind {
        UnderlineKind::Single => window.paint_underline(origin, width, &solid),
        UnderlineKind::Double => {
            window.paint_underline(point(origin.x, origin.y - px(1.)), width, &solid);
            window.paint_underline(point(origin.x, origin.y + px(1.)), width, &solid);
        }
        UnderlineKind::Curly => window.paint_underline(
            point(origin.x, origin.y - px(1.)),
            width,
            &UnderlineStyle {
                wavy: true,
                ..solid
            },
        ),
        UnderlineKind::Dotted => paint_patterned_line(
            window,
            row_origin_x,
            origin.x,
            origin.y,
            width,
            1.,
            2.,
            color,
        ),
        UnderlineKind::Dashed => paint_patterned_line(
            window,
            row_origin_x,
            origin.x,
            origin.y,
            width,
            3.,
            5.,
            color,
        ),
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TerminalPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.0).into();
        style.size.height = gpui::relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        // 文字の大きさと行の高さは見た目の設定から（O25）。等幅セル幅は 'M' を shape して測る。
        let appearance = self.terminal.read(cx).appearance.clone();
        let font_size = px(appearance.font_size);
        let font = window.text_style().font();
        let sample = window.text_system().shape_line(
            SharedString::from("M"),
            font_size,
            &[TextRun {
                len: 1,
                font: font.clone(),
                color: gpui::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let cell_width = if sample.width > px(0.) {
            sample.width
        } else {
            font_size * 0.6
        };
        let line_height = px(appearance.line_height());
        // 下線はベースラインの少し下（GPUI の文字の下線と同じ辺り）・二重下線が収まる高さまで。
        // 取り消し線は x 高さの真ん中。
        let font_id = window.text_system().resolve_font(&font);
        let ascent = window.text_system().ascent(font_id, font_size);
        let descent = px(f32::from(window.text_system().descent(font_id, font_size)).abs());
        let x_height = window.text_system().x_height(font_id, font_size);
        let baseline = (line_height - ascent - descent) / 2. + ascent;
        let underline_offset = (baseline + descent * 0.5).min(line_height - px(3.));
        let strikethrough_offset = baseline - x_height / 2.;

        // 行列サイズを算出して term をリサイズ。
        let columns = (f32::from(bounds.size.width) / f32::from(cell_width))
            .floor()
            .max(2.0) as usize;
        let lines = (f32::from(bounds.size.height) / f32::from(line_height))
            .floor()
            .max(1.0) as usize;
        term_probe(
            "layout",
            format_args!(
                "bounds {}x{} / cell {}x{} → {columns}列 {lines}行",
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
                f32::from(cell_width),
                f32::from(line_height),
            ),
        );
        let focused = self.terminal.read(cx).focus_handle.is_focused(window);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        self.terminal.update(cx, |terminal, _cx| {
            terminal.resize(columns, lines, cell_width, line_height);
            let frame = GridFrame {
                origin: bounds.origin,
                cell_width,
                line_height,
            };
            terminal.grid_frame = Some(frame);
            let content = &terminal.content;
            TerminalPrepaint {
                cells: content.cells.clone(),
                cursor: content.cursor,
                cursor_shape: content.cursor_shape,
                selection: content.selection,
                display_offset: content.display_offset,
                frame,
                underline_offset,
                strikethrough_offset,
                focused,
                theme: terminal.theme.clone(),
                colors: appearance.colors.clone(),
                links: content.links.clone(),
                hovered_link: terminal.hovered_link,
                search_matches: content.search_matches.clone(),
                current_match: content.current_match.clone(),
                columns,
                font_size,
                hitbox,
            }
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let theme = &prepaint.theme;
        let frame = prepaint.frame;
        let origin = frame.origin;
        let cell_width = frame.cell_width;
        let line_height = frame.line_height;
        let display_offset = prepaint.display_offset;
        // グリッド行（スクロールバック閲覧中は負値もある）→ 表示 y 座標。
        let row_y = |line: i32| origin.y + line_height * ((line + display_offset as i32) as f32);
        let cell_origin = |cell: AlacPoint| {
            point(
                origin.x + cell_width * (cell.column.0 as f32),
                row_y(cell.line.0),
            )
        };
        let cell_span = |cell: &RenderCell| {
            if cell.flags.contains(Flags::WIDE_CHAR) {
                cell_width * 2.
            } else {
                cell_width
            }
        };
        let font = window.text_style().font();

        // 面全体の背景。
        let colors = &prepaint.colors;
        let surface = background_hsla(theme, colors);
        window.paint_quad(fill(bounds, surface));

        // ① 既定でない背景セルの矩形。
        for cell in &prepaint.cells {
            let (mut foreground, mut background) = (cell.fg, cell.bg);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut foreground, &mut background);
            }
            if !is_default_background(background) {
                window.paint_quad(fill(
                    Bounds::new(cell_origin(cell.point), size(cell_width, line_height)),
                    ansi_to_hsla(background, theme, colors),
                ));
            }
        }

        // ② 検索の一致（エディタの ⌘F と同じ warn の薄塗り）。いま見ている一致は選択面の色を重ねる。
        let lines_visible = (f32::from(bounds.size.height) / f32::from(line_height)).ceil() as i32;
        let paint_match = |found: &Match, color: Hsla, window: &mut Window| {
            let (start, end) = (*found.start(), *found.end());
            for line in start.line.0..=end.line.0 {
                let row = line + display_offset as i32;
                if row < 0 || row >= lines_visible {
                    continue;
                }
                let first = if line == start.line.0 {
                    start.column.0
                } else {
                    0
                };
                let last = if line == end.line.0 {
                    end.column.0
                } else {
                    prepaint.columns.saturating_sub(1)
                };
                if last < first {
                    continue;
                }
                window.paint_quad(fill(
                    Bounds::new(
                        point(origin.x + cell_width * (first as f32), row_y(line)),
                        size(cell_width * ((last - first + 1) as f32), line_height),
                    ),
                    color,
                ));
            }
        };
        for found in &prepaint.search_matches {
            paint_match(found, theme.warn.alpha(0.16), window);
        }
        if let Some(found) = &prepaint.current_match {
            paint_match(found, theme_core::editor_selection(), window);
        }

        // ③ 選択（エディタと同じ選択面色）。背景の後に重ねる＝色付きの背景の上でも見える。
        if let Some(range) = prepaint.selection {
            for cell in &prepaint.cells {
                if range.contains(cell.point) {
                    window.paint_quad(fill(
                        Bounds::new(cell_origin(cell.point), size(cell_width, line_height)),
                        theme_core::editor_selection(),
                    ));
                }
            }
        }

        // ④ カーソル。アプリの指定した形（ブロック / 下線 / 縦棒）。フォーカスが無い時は
        // ブロックを輪郭にし、線の形は薄くする。Hidden（アプリが隠した）は描かない。
        // スクロールバック閲覧中は現在行が下へはみ出すので、面内にある時だけ描く。
        let mut filled_cursor = None;
        if let Some(cursor) = prepaint.cursor {
            let position = cell_origin(cursor);
            let inside = position.y >= bounds.origin.y
                && position.y + line_height <= bounds.origin.y + bounds.size.height;
            let width = prepaint
                .cells
                .iter()
                .find(|cell| cell.point == cursor)
                .map_or(cell_width, cell_span);
            let color = if prepaint.focused {
                theme.fg1
            } else {
                theme.fg2
            };
            if inside {
                match prepaint.cursor_shape {
                    CursorShape::Hidden => {}
                    CursorShape::Block if prepaint.focused => {
                        window.paint_quad(fill(
                            Bounds::new(position, size(width, line_height)),
                            theme.fg1,
                        ));
                        filled_cursor = Some(cursor);
                    }
                    CursorShape::Block | CursorShape::HollowBlock => {
                        window.paint_quad(gpui::outline(
                            Bounds::new(position, size(width, line_height)),
                            color,
                            gpui::BorderStyle::default(),
                        ));
                    }
                    CursorShape::Underline => window.paint_quad(fill(
                        Bounds::new(
                            point(position.x, position.y + line_height - px(2.)),
                            size(width, px(2.)),
                        ),
                        color,
                    )),
                    CursorShape::Beam => window.paint_quad(fill(
                        Bounds::new(position, size(px(2.), line_height)),
                        color,
                    )),
                }
            }
        }

        // ⑤ セル文字（1 セル 1 shape。v1 はバッチ無し）と装飾（下線・取り消し線）。
        for cell in &prepaint.cells {
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.flags.contains(Flags::HIDDEN) {
                continue;
            }
            // 反転（`\e[7m`）は文字色と背景色を入れ替える（既定の背景は面の色になる）。
            let foreground = if cell.flags.contains(Flags::INVERSE) {
                cell.bg
            } else {
                cell.fg
            };
            let mut color = ansi_to_hsla(foreground, theme, colors);
            if cell.flags.contains(Flags::DIM) {
                color = color.opacity(DIM_OPACITY);
            }
            // 塗りのカーソルの下の文字は視認性のため面の色で描く。
            if filled_cursor == Some(cell.point) {
                color = surface;
            }
            let position = cell_origin(cell.point);
            if cell.character != ' ' {
                let mut cell_font = font.clone();
                if cell.flags.contains(Flags::BOLD) {
                    cell_font.weight = gpui::FontWeight::BOLD;
                }
                if cell.flags.contains(Flags::ITALIC) {
                    cell_font.style = gpui::FontStyle::Italic;
                }
                let run = TextRun {
                    len: cell.character.len_utf8(),
                    font: cell_font,
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(
                    SharedString::from(cell.character.to_string()),
                    prepaint.font_size,
                    &[run],
                    Some(cell_width),
                );
                if let Err(error) = shaped.paint(
                    position,
                    line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                ) {
                    eprintln!("ターミナル文字描画に失敗: {error}");
                }
            }
            let width = cell_span(cell);
            if let Some(kind) = underline_kind(cell.flags) {
                let underline_color = cell
                    .underline_color
                    .map(|underline| ansi_to_hsla(underline, theme, colors))
                    .map(|underline| {
                        if cell.flags.contains(Flags::DIM) {
                            underline.opacity(DIM_OPACITY)
                        } else {
                            underline
                        }
                    })
                    .unwrap_or(color);
                paint_underline_kind(
                    window,
                    kind,
                    origin.x,
                    point(position.x, position.y + prepaint.underline_offset),
                    width,
                    underline_color,
                );
            }
            if cell.flags.contains(Flags::STRIKEOUT) {
                window.paint_strikethrough(
                    point(position.x, position.y + prepaint.strikethrough_offset),
                    width,
                    &StrikethroughStyle {
                        thickness: px(1.),
                        color: Some(color),
                    },
                );
            }
        }

        // ⑥ リンク（OSC 8・URL・`path:line`）の下線。ポインタが載っている間は濃くして指の形。
        for (index, link) in prepaint.links.iter().enumerate() {
            let hovered = prepaint.hovered_link == Some(index);
            let start = point(
                origin.x + cell_width * (link.columns.start as f32),
                row_y(link.line) + prepaint.underline_offset,
            );
            window.paint_underline(
                start,
                cell_width * (link.columns.len() as f32),
                &UnderlineStyle {
                    thickness: px(1.),
                    color: Some(if hovered { theme.fg0 } else { theme.fg2 }),
                    wavy: false,
                },
            );
        }
        let cursor_style = if prepaint.hovered_link.is_some() {
            CursorStyle::PointingHand
        } else {
            CursorStyle::IBeam
        };
        window.set_cursor_style(cursor_style, &prepaint.hitbox);

        // ⑦ マウス（選択・リンク）。判定と状態は TerminalView が持ち、ここはこのフレームの座標を渡すだけ。
        {
            let terminal = self.terminal.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !bounds.contains(&event.position) {
                    return;
                }
                terminal.update(cx, |terminal, cx| {
                    terminal.on_grid_mouse_down(event, frame, window, cx)
                });
            });
        }
        {
            let terminal = self.terminal.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let inside = bounds.contains(&event.position);
                terminal.update(cx, |terminal, cx| {
                    terminal.on_grid_mouse_move(event, frame, inside, cx)
                });
            });
        }
        {
            let terminal = self.terminal.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                terminal.update(cx, |terminal, cx| {
                    terminal.on_grid_mouse_up(event, frame, cx)
                });
            });
        }

        // ⑧ IME 入力ハンドラ（M13）: 日本語などの確定文字列を PTY へ流せるようにする。
        window.handle_input(
            &self.terminal.read(cx).focus_handle,
            ElementInputHandler::new(bounds, self.terminal.clone()),
            cx,
        );
    }
}
