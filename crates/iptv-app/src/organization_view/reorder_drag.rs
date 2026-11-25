//! The list viewport owns grip gestures across scrolling and cell recycling.
use super::{
    ROW, Scale, Ui, contents_hidden, list_rows, order_outline, reordering, state, ui_kit, update,
};
use nsh_core::prelude::*;
use std::time::Duration;

pub(crate) const GRIP_COLUMN: f32 = 56.0;
const EDGE: f32 = 44.0;
const MAX_SCROLL_STEP: f32 = 22.0;

#[derive(Clone, Debug, PartialEq)]
struct Drag {
    from: usize,
    id: String,
    origin_y: f32,
    y: f32,
    grab_offset: f32,
    name: String,
}

#[derive(Clone, Copy)]
pub(crate) struct DragList {
    ui: Ui,
    size: Scale,
    rows: ElementRef,
    active: Signal<Option<Drag>>,
    scroll: Signal<f32>,
    viewport: Signal<Option<Bounds<Points>>>,
    column: Option<fn() -> f32>,
}

impl DragList {
    pub(crate) fn new(ui: Ui, size: Scale, rows: ElementRef) -> Self {
        Self {
            ui,
            size,
            rows,
            active: signal(None),
            scroll: signal(0.0),
            viewport: signal(None),
            column: None,
        }
    }

    pub(crate) fn guide(mut self, column: fn() -> f32) -> Self {
        self.column = Some(column);
        self
    }

    pub(crate) fn active(self) -> bool {
        self.active.with(Option::is_some)
    }

    fn pitch(self) -> f32 {
        if self.column.is_some() {
            self.size.whole(crate::list_rows::ROW)
        } else {
            list_rows::pitch(self.size, ROW)().0
        }
    }

    pub(crate) fn scrolled(self, event: ScrollEvent) {
        if self.scroll.get_untracked() == event.offset.y.0 {
            return;
        }
        self.scroll.set(event.offset.y.0);
        self.move_row();
    }

    fn start(self, y: f32) {
        let position = (y + self.scroll.get_untracked()).max(0.0);
        let pitch = self.pitch();
        let from = (position / pitch).floor() as usize;
        let mut grabbed = None;
        self.ui.touched();
        batch(|| {
            update(self.ui, |screen| {
                if screen.saving {
                    return;
                }
                let Some(order) = screen.editor.reorder.as_mut() else {
                    return;
                };
                let Some(item) = order.items().get(from).cloned() else {
                    return;
                };
                if order.selected().is_some() {
                    order.toggle_selection();
                }
                order.focus_on(&item);
                order.toggle_selection();
                screen.dragging = true;
                grabbed = Some(item);
            });
            if let Some(item) = grabbed {
                self.active.set(Some(Drag {
                    from,
                    id: item.id,
                    origin_y: y,
                    y,
                    grab_offset: position - from as f32 * pitch,
                    name: item.name,
                }));
            }
        });
    }

    fn dragged(self, delta_y: f32) {
        self.active.update(|drag| {
            if let Some(drag) = drag {
                drag.y = drag.origin_y + delta_y;
            }
        });
        self.move_row();
    }

    fn move_row(self) {
        self.active.with_untracked(|drag| {
            let Some(drag) = drag else { return };
            let top = drag.y + self.scroll.get_untracked() - drag.grab_offset;
            let to = (top / self.pitch()).round().max(0.0) as usize;
            update(self.ui, |screen| {
                if !screen.saving
                    && let Some(order) = screen.editor.reorder.as_mut()
                    && order.selected().is_some_and(|item| item.id == drag.id)
                {
                    order.move_selected_to(to);
                }
            });
        });
    }

    fn finish(self, canceled: bool) {
        let mut drag = None;
        self.active.update(|active| drag = active.take());
        let Some(drag) = drag else { return };
        update(self.ui, |screen| {
            screen.dragging = false;
            if let Some(order) = screen.editor.reorder.as_mut()
                && order.selected().is_some_and(|item| item.id == drag.id)
            {
                if canceled {
                    order.move_selected_to(drag.from);
                }
                order.toggle_selection();
            }
        });
    }

    fn scroll_at_edge(self) {
        let Some(y) = self
            .active
            .with_untracked(|drag| drag.as_ref().map(|drag| drag.y))
        else {
            return;
        };
        let Some(viewport) = self.viewport.get_untracked() else {
            return;
        };
        let height = viewport.size.height.0;
        let edge = self.size.of(EDGE).min(height / 3.0);
        if edge <= 0.0 {
            return;
        }
        let step =
            ((y - height + edge) / edge).clamp(0.0, 1.0) - ((edge - y) / edge).clamp(0.0, 1.0);
        if step == 0.0 {
            return;
        }
        let count = state(self.ui, |s| s.editor.rows().len()).unwrap_or(0);
        let max = (count as f32 * self.pitch() - height).max(0.0);
        let current = self.scroll.get_untracked();
        let next = (current + step * MAX_SCROLL_STEP).clamp(0.0, max);
        if next != current {
            self.rows.scroll_to_offset(px(next), false);
        }
    }

    fn can_grab(self) -> bool {
        self.ui.drag_editing()
            && reordering(self.ui)
            && !contents_hidden(self.ui)
            && (self.column.is_none() || super::guide_order_accessible(self.ui))
    }

    pub(crate) fn with_grips(self, content: View) -> View {
        let size = self.size;
        let viewport = element_ref();
        effect(move || {
            let closed = self.column.is_some() && !super::guide_order_accessible(self.ui);
            if (closed || contents_hidden(self.ui)) && self.active() {
                untracked(|| self.finish(closed));
            }
        });
        content
            .element_ref(&viewport)
            .on_layout(move |_: &Bounds<Points>| {
                drop(spawn(async move {
                    if let Some(layout) = viewport.measure().await {
                        self.viewport.set(Some(Bounds {
                            origin: layout.page,
                            size: layout.frame.size,
                        }));
                    }
                }));
            })
            .claim_touches(
                TouchClaim::on_start()
                    .capture()
                    .ask(move |request: &ClaimRequest| {
                        if !self.can_grab() {
                            return;
                        }
                        if let Some(viewport) = self.viewport.get_untracked() {
                            let x = request.event.page.x - viewport.origin.x;
                            let right = self
                                .column
                                .map_or(viewport.size.width, |column| px(column()));
                            if x >= right - self.size.px(GRIP_COLUMN) && x <= right {
                                request.claim();
                            }
                        }
                    }),
            )
            .keep_touches(self.column.is_some())
            .on_responder_grant(move |event: &ResponderEvent| {
                if let Some(viewport) = self.viewport.get_untracked() {
                    self.start((event.page.y - viewport.origin.y).0);
                }
            })
            .on_responder_move(move |e: &ResponderEvent| self.dragged(e.gesture.delta.y.0))
            .on_responder_release(move |_: &ResponderEvent| self.finish(false))
            .on_responder_terminate(move |_: &ResponderEvent| self.finish(true))
            .child(when(
                move || crate::host::ANDROID && self.column.is_none() && self.can_grab(),
                move || {
                    // Keep the grip strip outside the native scroll view's touch path.
                    view()
                        .absolute()
                        .right(px(0.0))
                        .top(px(0.0))
                        .bottom(px(0.0))
                        .w(move || size.px(GRIP_COLUMN))
                        .focusable(false)
                        .accessibility_elements_hidden(true)
                        .on_press(|| {})
                },
            ))
            .child(when(
                move || self.active(),
                move || {
                    let _timer = set_interval(Duration::from_millis(24), move || {
                        self.scroll_at_edge();
                    });
                    view()
                        .absolute()
                        .left(px(0.0))
                        .right(move || {
                            self.column.map_or(px(0.0), |column| {
                                px(self.viewport.with(|bounds| {
                                    bounds.map_or(0.0, |bounds| {
                                        (bounds.size.width.0 - column()).max(0.0)
                                    })
                                }))
                            })
                        })
                        .top(move || {
                            self.active.with(|drag| {
                                px(drag.as_ref().map_or(0.0, |drag| drag.y - drag.grab_offset))
                            })
                        })
                        .h(move || px(self.pitch()))
                        .px(move || size.px(list_rows::INSET))
                        .rounded(px(crate::theme::RADIUS))
                        .bg(move || rgb(self.ui.app.theme.with(|t| t.background.panel)))
                        .pointer_events(PointerEvents::None)
                        .accessibility_elements_hidden(true)
                        .flex_row()
                        .items_center()
                        .child(
                            ui_kit::label(size, move || {
                                self.active.with(|drag| {
                                    drag.as_ref()
                                        .map(|drag| drag.name.clone())
                                        .unwrap_or_default()
                                })
                            })
                            .flex_1(),
                        )
                        .child(grip(size))
                        .child(order_outline(self.ui, || true))
                },
            ))
    }
}

pub(super) fn handle(size: Scale, row: usize) -> impl IntoElement {
    grip(size)
        .cursor(Cursor::Grab)
        .test_id(&format!("reorder-handle-{row}"))
}

pub(crate) fn grip(size: Scale) -> View {
    view()
        .w(move || size.px(GRIP_COLUMN - 2.0 * list_rows::INSET))
        .gap(px(3.0))
        .items_center()
        .justify_center()
        .accessibility_elements_hidden(true)
        .children((0..3).map(|_| {
            view()
                .w(px(14.0))
                .h(px(2.0))
                .rounded(px(1.0))
                .bg(ui_kit::secondary())
        }))
}
