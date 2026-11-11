//! A settings row that keeps a label, a control, and an optional note on one
//! line while they fit, and folds them onto lines of their own when they
//! don't, instead of squeezing the control. A plain row hands a control
//! whatever the label leaves, so at a big interface size a dropdown shrinks
//! to a few pixels.
//!
//! iced's wrapping row can't do this on its own: it offers every child the
//! whole width, so a control that fills would always take a line to itself,
//! and a note that has to drop would land under the label, not the control.
//!
//! The control is either fixed wide, or it fills. A fixed control that fits
//! beside the label stays there; the note follows on the same line when it
//! fits, and otherwise sits under the control. A fixed control that doesn't
//! fit goes under the label, with the note after it. A control that fills
//! stays beside the label while it keeps `min_control` of room, and folds
//! under the label below that.

use iced::advanced::widget::{Operation, tree::Tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// Between a label, its control, and its note on a line.
const SPACING: f32 = 12.0;
/// Between lines, closer than the gap between rows.
const LINE_GAP: f32 = 6.0;

/// `children` are the label, the control, and the note if any. The control
/// must be fixed wide or fill.
pub(crate) fn fold_row<'a, Message: 'a>(
    label: Element<'a, Message>,
    control: Element<'a, Message>,
    note: Option<Element<'a, Message>>,
    min_control: f32,
) -> Element<'a, Message> {
    let fills = control.as_widget().size().width.fill_factor() > 0;
    let mut children = vec![label, control];
    children.extend(note);
    Element::new(FoldRow {
        children,
        fills,
        min_control,
        max_height: f32::INFINITY,
    })
}

struct FoldRow<'a, Message> {
    children: Vec<Element<'a, Message>>,
    fills: bool,
    min_control: f32,
    /// The height the parent allows, set at layout.
    max_height: f32,
}

/// Where a laid out child goes, by its index.
struct Place {
    index: usize,
    x: f32,
    line: usize,
}

impl<Message> FoldRow<'_, Message> {
    fn lay_out(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        index: usize,
        width: f32,
    ) -> layout::Node {
        self.children[index].as_widget_mut().layout(
            &mut tree.children[index],
            renderer,
            // Compressed in height, so a child that fills it takes its own
            // height, as in a plain row, not the parent's whole limit.
            &layout::Limits::with_compression(
                Size::ZERO,
                Size::new(width.max(0.0), self.max_height),
                Size::new(false, true),
            ),
        )
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for FoldRow<'_, Message> {
    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(
            if self.fills {
                Length::Fill
            } else {
                Length::Shrink
            },
            Length::Shrink,
        )
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let room = limits.max().width;
        self.max_height = limits.max().height;
        let has_note = self.children.len() == 3;
        let label = self.lay_out(tree, renderer, 0, room);
        let label_width = label.size().width;
        let beside = label_width + SPACING;

        // Which line each child is on and where along it, then the widths
        // the control and the note are laid out with.
        let control_room;
        let mut places = vec![Place {
            index: 0,
            x: 0.0,
            line: 0,
        }];
        let note_natural = if has_note {
            self.lay_out(tree, renderer, 2, room).size().width
        } else {
            0.0
        };
        let note_gap = if has_note { SPACING } else { 0.0 };

        if !room.is_finite() {
            // No limit to fold against, so everything sits on one line.
            control_room = if self.fills {
                self.min_control
            } else {
                f32::INFINITY
            };
            let control_width = self.lay_out(tree, renderer, 1, control_room).size().width;
            places.push(Place {
                index: 1,
                x: beside,
                line: 0,
            });
            if has_note {
                places.push(Place {
                    index: 2,
                    x: beside + control_width + SPACING,
                    line: 0,
                });
            }
        } else if self.fills {
            // The control takes what the label and the note leave.
            let left = room - beside - note_gap - note_natural;
            if left + 0.01 >= self.min_control {
                control_room = left;
                places.push(Place {
                    index: 1,
                    x: beside,
                    line: 0,
                });
                if has_note {
                    places.push(Place {
                        index: 2,
                        x: beside + left + SPACING,
                        line: 0,
                    });
                }
            } else {
                // Under the label, with the note beside it if the control
                // keeps its minimum, and under it if not.
                let left = room - note_gap - note_natural;
                let keeps = left + 0.01 >= self.min_control.min(room);
                control_room = if has_note && keeps { left } else { room };
                places.push(Place {
                    index: 1,
                    x: 0.0,
                    line: 1,
                });
                if has_note {
                    places.push(if keeps {
                        Place {
                            index: 2,
                            x: left + SPACING,
                            line: 1,
                        }
                    } else {
                        Place {
                            index: 2,
                            x: 0.0,
                            line: 2,
                        }
                    });
                }
            }
        } else {
            let control_width = self.lay_out(tree, renderer, 1, room).size().width;
            control_room = room;
            if beside + control_width <= room + 0.01 {
                places.push(Place {
                    index: 1,
                    x: beside,
                    line: 0,
                });
                if has_note {
                    if beside + control_width + note_gap + note_natural <= room + 0.01 {
                        places.push(Place {
                            index: 2,
                            x: beside + control_width + SPACING,
                            line: 0,
                        });
                    } else {
                        // Under the control.
                        places.push(Place {
                            index: 2,
                            x: beside,
                            line: 1,
                        });
                    }
                }
            } else {
                places.push(Place {
                    index: 1,
                    x: 0.0,
                    line: 1,
                });
                if has_note {
                    if control_width + note_gap + note_natural <= room + 0.01 {
                        places.push(Place {
                            index: 2,
                            x: control_width + SPACING,
                            line: 1,
                        });
                    } else {
                        places.push(Place {
                            index: 2,
                            x: 0.0,
                            line: 2,
                        });
                    }
                }
            }
        }

        let mut nodes: Vec<(usize, layout::Node, f32, usize)> = vec![(0, label, 0.0, 0)];
        for place in places.iter().skip(1) {
            let width = if place.index == 1 {
                control_room
            } else {
                room - place.x
            };
            let node = self.lay_out(tree, renderer, place.index, width);
            nodes.push((place.index, node, place.x, place.line));
        }

        // Lines are as tall as their tallest child, with the children
        // centered on them.
        let lines = nodes.iter().map(|node| node.3).max().unwrap_or(0) + 1;
        let mut heights = vec![0.0_f32; lines];
        for (_, node, _, line) in &nodes {
            heights[*line] = heights[*line].max(node.size().height);
        }
        let mut tops = Vec::with_capacity(lines);
        let mut top = 0.0;
        for height in &heights {
            tops.push(top);
            top += height + LINE_GAP;
        }
        let mut content = Size::new(0.0, (top - LINE_GAP).max(0.0));
        let mut placed: Vec<(usize, layout::Node)> = Vec::new();
        for (index, node, x, line) in nodes {
            let size = node.size();
            content.width = content.width.max(x + size.width);
            let y = tops[line] + (heights[line] - size.height) / 2.0;
            placed.push((index, node.move_to((x, y))));
        }
        placed.sort_by_key(|(index, _)| *index);
        let width = if self.fills && room.is_finite() {
            content.width = room;
            Length::Fill
        } else {
            Length::Shrink
        };
        let size = limits.resolve(width, Length::Shrink, content);
        layout::Node::with_children(size, placed.into_iter().map(|(_, node)| node).collect())
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            self.children
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
                .for_each(|((child, state), layout)| {
                    child
                        .as_widget_mut()
                        .operate(state, layout, renderer, operation);
                });
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for ((child, tree), layout) in self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            child.as_widget_mut().update(
                tree, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, tree), layout)| {
                child
                    .as_widget()
                    .mouse_interaction(tree, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for ((child, tree), layout) in self
            .children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .filter(|(_, layout)| layout.bounds().intersects(viewport))
        {
            child
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        overlay::from_children(
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_probe::layout;
    use iced::Fill;
    use iced::widget::Space;

    fn block(width: impl Into<Length>, height: f32) -> Element<'static, ()> {
        Space::new().width(width).height(height).into()
    }

    /// The origin of the label and the bounds of the other children, in a
    /// row `room` wide.
    fn place(
        control: Element<'static, ()>,
        note: Option<Element<'static, ()>>,
        room: f32,
    ) -> Vec<Rectangle> {
        let mut row = fold_row(block(180, 20.0), control, note, 260.0);
        let node = layout(&mut row, Size::new(room, 400.0));
        node.children().iter().map(|child| child.bounds()).collect()
    }

    #[test]
    fn a_fixed_control_sits_beside_its_label_down_to_an_exact_fit() {
        for room in [700.0, 452.0] {
            let parts = place(block(260, 30.0), None, room);
            assert_eq!((parts[1].x, parts[1].width), (192.0, 260.0), "{room}");
            // Centered on the taller of the two.
            assert_eq!((parts[0].y, parts[1].y), (5.0, 0.0), "{room}");
        }
        let parts = place(block(260, 30.0), None, 451.0);
        assert_eq!((parts[1].x, parts[1].y), (0.0, 26.0));
    }

    #[test]
    fn a_fixed_control_shrinks_to_a_room_narrower_than_itself() {
        let parts = place(block(260, 30.0), None, 200.0);
        assert_eq!((parts[1].x, parts[1].width), (0.0, 200.0));
    }

    #[test]
    fn a_note_stays_on_the_line_until_it_runs_out_of_room() {
        // 192 + 260 + 12 + 60 = 524.
        let parts = place(block(260, 30.0), Some(block(60, 20.0)), 524.0);
        assert_eq!((parts[2].x, parts[2].y), (464.0, 5.0));
        let parts = place(block(260, 30.0), Some(block(60, 20.0)), 523.0);
        assert_eq!(
            (parts[1].x, parts[2].x),
            (192.0, 192.0),
            "under the control"
        );
        assert_eq!(parts[2].y, 36.0);
    }

    #[test]
    fn a_folded_control_keeps_its_note_after_it_and_then_under_it() {
        // The control is 260 wide, so 60 more fits in 332 and not in 331.
        let parts = place(block(260, 30.0), Some(block(60, 20.0)), 332.0);
        assert_eq!((parts[1].x, parts[2].x, parts[2].y), (0.0, 272.0, 31.0));
        let parts = place(block(260, 30.0), Some(block(60, 20.0)), 331.0);
        assert_eq!((parts[1].x, parts[2].x), (0.0, 0.0));
        assert_eq!(parts[2].y, 26.0 + 30.0 + 6.0);
    }

    #[test]
    fn a_filling_control_takes_the_rest_down_to_a_control_width() {
        let parts = place(block(Fill, 30.0), None, 700.0);
        assert_eq!((parts[1].x, parts[1].width), (192.0, 508.0));
        let parts = place(block(Fill, 30.0), None, 452.0);
        assert_eq!((parts[1].x, parts[1].width), (192.0, 260.0));
        let parts = place(block(Fill, 30.0), None, 451.0);
        assert_eq!((parts[1].x, parts[1].width, parts[1].y), (0.0, 451.0, 26.0));
    }

    #[test]
    fn a_filling_control_counts_its_note_against_the_room_beside_the_label() {
        // The note takes 72, so the field has 260 left at 524.
        let parts = place(block(Fill, 30.0), Some(block(60, 20.0)), 524.0);
        assert_eq!(
            (parts[1].x, parts[1].width, parts[2].x),
            (192.0, 260.0, 464.0)
        );
        // Folded, the note stays beside the field while the field keeps a
        // control width, and drops under it otherwise.
        let parts = place(block(Fill, 30.0), Some(block(60, 20.0)), 400.0);
        assert_eq!(
            (parts[1].x, parts[1].width, parts[2].x),
            (0.0, 328.0, 340.0)
        );
        let parts = place(block(Fill, 30.0), Some(block(60, 20.0)), 300.0);
        assert_eq!((parts[1].width, parts[2].x), (300.0, 0.0));
        assert!(parts[2].y >= parts[1].y + parts[1].height);
    }

    #[test]
    fn a_fill_control_with_a_note_keeps_its_note_beside_it_down_to_a_control_width() {
        // Folded under the label, the field needs 260 and the note 72 more.
        let parts = place(block(Fill, 30.0), Some(block(60, 20.0)), 332.0);
        assert_eq!((parts[1].x, parts[1].width, parts[1].y), (0.0, 260.0, 26.0));
        assert_eq!((parts[2].x, parts[2].y), (272.0, 31.0));
        let parts = place(block(Fill, 30.0), Some(block(60, 20.0)), 331.0);
        assert_eq!(
            (parts[1].width, parts[2].x),
            (331.0, 0.0),
            "under the field"
        );
        assert_eq!(parts[2].y, 26.0 + 30.0 + 6.0);
    }

    #[test]
    fn an_unbounded_room_puts_everything_on_one_line() {
        let room = f32::INFINITY;
        let parts = place(block(260, 30.0), Some(block(60, 20.0)), room);
        assert_eq!((parts[1].x, parts[2].x), (192.0, 464.0));
        let parts = place(block(Fill, 30.0), Some(block(60, 20.0)), room);
        assert_eq!(
            (parts[1].x, parts[1].width, parts[2].x),
            (192.0, 260.0, 464.0)
        );
        let mut row = fold_row(block(180, 20.0), block(Fill, 30.0), None, 260.0);
        let size = layout(&mut row, Size::new(room, 400.0)).size();
        assert_eq!(size, Size::new(452.0, 30.0));
    }

    #[test]
    fn a_child_that_fills_its_height_takes_its_own_height_not_the_limit() {
        // As in a plain row, so a row in a tall page doesn't grow to fit it.
        let tall: Element<'static, ()> = Space::new().width(260).height(Fill).into();
        let mut row = fold_row(block(180, 20.0), tall, None, 260.0);
        let size = layout(&mut row, Size::new(700.0, 400.0)).size();
        assert_eq!(size.height, 20.0);
    }
}
