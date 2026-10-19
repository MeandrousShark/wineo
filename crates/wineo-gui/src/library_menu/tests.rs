use iced::advanced::widget::Operation;
use iced::advanced::widget::operation::{Focusable, Outcome, Scrollable};
use iced::widget::scrollable::{AbsoluteOffset, RelativeOffset};
use iced::{Point, Size};

use super::*;

const MENU: &str = "menu";

/// A widget tree as operations see it.
enum Node {
    /// A scrollable with its id, viewport, content height, offset, and
    /// children, laid out from the top of its viewport.
    Scroll(&'static str, Rectangle, f32, f32, Vec<Node>),
    /// A focusable control and whether it has focus.
    Focus(Rectangle, bool),
    Group(Vec<Node>),
}

struct FakeScroll<'a>(&'a mut Option<f32>);

impl Scrollable for FakeScroll<'_> {
    fn snap_to(&mut self, _offset: RelativeOffset<Option<f32>>) {}

    fn scroll_to(&mut self, offset: AbsoluteOffset<Option<f32>>) {
        *self.0 = offset.y;
    }

    fn scroll_by(&mut self, _offset: AbsoluteOffset, _bounds: Rectangle, _content: Rectangle) {}
}

struct FakeFocus(bool);

impl Focusable for FakeFocus {
    fn is_focused(&self) -> bool {
        self.0
    }

    fn focus(&mut self) {}

    fn unfocus(&mut self) {}
}

/// Walks `node` the way iced's widgets do: a scrollable reports itself
/// and then traverses its content.
fn walk(operation: &mut dyn Operation, node: &Node, scrolled: &mut Option<f32>) {
    match node {
        Node::Scroll(id, viewport, height, offset, children) => {
            let content = Rectangle::new(viewport.position(), Size::new(viewport.width, *height));
            operation.scrollable(
                Some(&iced::widget::Id::new(id)),
                *viewport,
                content,
                iced::Vector::new(0.0, *offset),
                &mut FakeScroll(scrolled),
            );
            operation.traverse(&mut |operation| {
                for child in children {
                    walk(operation, child, scrolled);
                }
            });
        }
        Node::Focus(bounds, focused) => {
            operation.focusable(None, *bounds, &mut FakeFocus(*focused));
        }
        Node::Group(children) => operation.traverse(&mut |operation| {
            for child in children {
                walk(operation, child, scrolled);
            }
        }),
    }
}

/// Where revealing the focused control scrolls the menu, if anywhere.
fn reveal(tree: &Node) -> Option<f32> {
    let mut scrolled = None;
    let mut operation: Box<dyn Operation> = Box::new(reveal_focused(MENU));
    loop {
        walk(operation.as_mut(), tree, &mut scrolled);
        match operation.finish() {
            Outcome::Chain(next) => operation = next,
            _ => return scrolled,
        }
    }
}

fn row(y: f32, height: f32, focused: bool) -> Node {
    Node::Focus(
        Rectangle::new(Point::new(0.0, y), Size::new(200.0, height)),
        focused,
    )
}

/// A menu whose viewport is `viewport` tall at the window's top, scrolled
/// to `offset`, beside a control outside it.
fn menu(viewport: f32, offset: f32, rows: Vec<Node>, outside: Node) -> Node {
    Node::Group(vec![
        Node::Scroll(
            MENU,
            Rectangle::new(Point::ORIGIN, Size::new(200.0, viewport)),
            400.0,
            offset,
            rows,
        ),
        outside,
    ])
}

#[test]
fn a_focused_row_below_the_view_scrolls_up_to_show_it() {
    let rows = vec![row(40.0, 32.0, false), row(200.0, 32.0, true)];
    let tree = menu(100.0, 0.0, rows, row(500.0, 32.0, false));
    assert_eq!(reveal(&tree), Some(132.0));
}

#[test]
fn focus_outside_the_menu_never_scrolls_it() {
    // Beside the menu, in the same rows of the window.
    let rows = vec![row(40.0, 32.0, false), row(200.0, 32.0, false)];
    let tree = menu(100.0, 50.0, rows, row(200.0, 32.0, true));
    assert_eq!(reveal(&tree), None);
    // An outside control above the menu's first row doesn't make the
    // focused row look like a later one, either.
    let rows = vec![row(40.0, 32.0, true), row(200.0, 32.0, false)];
    let tree = menu(100.0, 50.0, rows, row(0.0, 32.0, false));
    assert_eq!(reveal(&tree), Some(0.0));
}

#[test]
fn the_first_row_goes_to_the_top_only_when_it_fits_there() {
    let rows = vec![row(40.0, 32.0, true), row(200.0, 32.0, false)];
    let tree = menu(100.0, 150.0, rows, Node::Group(Vec::new()));
    assert_eq!(reveal(&tree), Some(0.0));

    // A viewport too short to show the header and the row together shows
    // the row like any other.
    let rows = vec![row(40.0, 32.0, true), row(200.0, 32.0, false)];
    let tree = menu(20.0, 150.0, rows, Node::Group(Vec::new()));
    assert_eq!(reveal(&tree), Some(40.0));
    let rows = vec![row(40.0, 32.0, true), row(200.0, 32.0, false)];
    let tree = menu(50.0, 150.0, rows, Node::Group(Vec::new()));
    assert_eq!(reveal(&tree), Some(40.0));
}

#[test]
fn a_row_taller_than_the_view_shows_its_top_every_time() {
    let tall = || vec![row(10.0, 32.0, false), row(90.0, 30.0, true)];
    let first = reveal(&menu(20.0, 0.0, tall(), Node::Group(Vec::new())));
    assert_eq!(first, Some(90.0));
    // Revealed again from there, it stays put rather than alternating.
    assert_eq!(
        reveal(&menu(20.0, 90.0, tall(), Node::Group(Vec::new()))),
        None
    );
    assert_eq!(
        reveal(&menu(20.0, 80.0, tall(), Node::Group(Vec::new()))),
        Some(90.0)
    );
}
