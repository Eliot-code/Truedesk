//! Turns the viewer's mouse and keyboard into what a phone understands:
//! touch gestures, text edits and the system Back/Home/Recents buttons. The
//! Android app's accessibility service carries these out; it takes them from
//! an [`ActionQueue`] one at a time.
//!
//! Pointer positions travel as fractions of the screen (0.0..=1.0), so the
//! app can place them on the real display whatever size the video is.

use host_core::inject::InputInjector;
use protocol::input::InputEvent;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Something for the phone to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// A one-finger touch along `points`, held for `ms` milliseconds: a tap
    /// when it stays put and is short, a long press when it stays put and is
    /// long, a drag otherwise.
    Stroke { points: Vec<(f32, f32)>, ms: u32 },
    /// Scroll the content under `(x, y)` by `(dx, dy)` screens; positive `dy`
    /// shows content further down, like turning a mouse wheel towards you.
    Scroll { x: f32, y: f32, dx: f32, dy: f32 },
    /// Type `text` into the focused field, replacing any selection.
    Text(String),
    /// An editing key in the focused field.
    Edit(Edit),
    /// A system button.
    Global(Global),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Backspace,
    Delete,
    Enter,
    Left,
    Right,
    Home,
    End,
    SelectAll,
    Copy,
    Cut,
    Paste,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Global {
    Back,
    Home,
    Recents,
    Notifications,
}

impl Action {
    /// The action as one line of JSON for the app, e.g.
    /// `{"t":"stroke","ms":80,"p":[[0.5,0.25]]}`.
    pub fn to_json(&self) -> String {
        match self {
            Self::Stroke { points, ms } => {
                let mut out = format!(r#"{{"t":"stroke","ms":{ms},"p":["#);
                for (index, (x, y)) in points.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    let _ = write!(out, "[{},{}]", fraction(*x), fraction(*y));
                }
                out.push_str("]}");
                out
            }
            Self::Scroll { x, y, dx, dy } => format!(
                r#"{{"t":"scroll","x":{},"y":{},"dx":{},"dy":{}}}"#,
                fraction(*x), fraction(*y), signed(*dx), signed(*dy)
            ),
            Self::Text(text) => format!(r#"{{"t":"text","s":{}}}"#, json_string(text)),
            Self::Edit(edit) => format!(r#"{{"t":"edit","k":"{}"}}"#, edit.name()),
            Self::Global(global) => format!(r#"{{"t":"global","k":"{}"}}"#, global.name()),
        }
    }
}

impl Edit {
    fn name(self) -> &'static str {
        match self {
            Self::Backspace => "backspace",
            Self::Delete => "delete",
            Self::Enter => "enter",
            Self::Left => "left",
            Self::Right => "right",
            Self::Home => "home",
            Self::End => "end",
            Self::SelectAll => "select_all",
            Self::Copy => "copy",
            Self::Cut => "cut",
            Self::Paste => "paste",
        }
    }
}

impl Global {
    fn name(self) -> &'static str {
        match self {
            Self::Back => "back",
            Self::Home => "home",
            Self::Recents => "recents",
            Self::Notifications => "notifications",
        }
    }
}

fn fraction(value: f32) -> String {
    format!("{:.4}", value.clamp(0.0, 1.0))
}

fn signed(value: f32) -> String {
    format!("{:.4}", value.clamp(-1.0, 1.0))
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Most actions waiting for the app. The app normally keeps up; when it
/// doesn't (no accessibility service yet), the oldest are dropped rather
/// than replayed long after the viewer gave up on them.
pub const MAX_QUEUED: usize = 256;
/// Most points kept for one stroke. A long drag keeps its first points and
/// its latest; the gesture still ends where the pointer was let go.
pub const MAX_STROKE_POINTS: usize = 200;
/// Longest gesture Android accepts, less a margin.
pub const MAX_STROKE_MS: u32 = 59_000;
/// One wheel notch scrolls this much of the screen.
pub const NOTCH: f32 = 0.1;
/// One scroll action moves at most this much of the screen.
pub const MAX_SCROLL: f32 = 0.6;
/// Pointer moves smaller than this (a fraction of the screen) don't add a
/// point to the stroke.
const POINT_SLOP: f32 = 0.002;

/// Actions waiting for the app, shared by the injector (on the session's
/// injection thread) and the app's accessibility service (on its own thread).
#[derive(Clone, Default)]
pub struct ActionQueue {
    inner: Arc<(Mutex<VecDeque<Action>>, Condvar)>,
}

impl ActionQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues an action, merging it into the last one where that keeps the
    /// result the same: typed text joins the text before it, and a scroll at
    /// the same spot adds to the scroll before it.
    pub fn push(&self, action: Action) {
        let (queue, ready) = &*self.inner;
        let mut queue = queue.lock().unwrap_or_else(|p| p.into_inner());
        match (queue.back_mut(), action) {
            (Some(Action::Text(last)), Action::Text(text)) => last.push_str(&text),
            (Some(Action::Scroll { x, y, dx, dy }), Action::Scroll { x: nx, y: ny, dx: ndx, dy: ndy })
                if *x == nx && *y == ny =>
            {
                *dx = (*dx + ndx).clamp(-MAX_SCROLL, MAX_SCROLL);
                *dy = (*dy + ndy).clamp(-MAX_SCROLL, MAX_SCROLL);
            }
            (_, action) => {
                if queue.len() == MAX_QUEUED {
                    queue.pop_front();
                }
                queue.push_back(action);
            }
        }
        ready.notify_one();
    }

    /// The next action, waiting up to `timeout` for one.
    pub fn next(&self, timeout: Duration) -> Option<Action> {
        let (queue, ready) = &*self.inner;
        let queue = queue.lock().unwrap_or_else(|p| p.into_inner());
        let (mut queue, _) = ready
            .wait_timeout_while(queue, timeout, |queue| queue.is_empty())
            .unwrap_or_else(|p| p.into_inner());
        queue.pop_front()
    }

    /// Drops everything waiting, e.g. when a session ends.
    pub fn clear(&self) {
        self.inner.0.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    pub fn len(&self) -> usize {
        self.inner.0.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Protocol modifier bits (see `viewer/src/lib/input.ts`).
const CTRL: u8 = 1 << 1;
const ALT: u8 = 1 << 2;
const META: u8 = 1 << 3;

/// The [`InputInjector`] of a phone host: maps viewer input to [`Action`]s.
pub struct GestureInjector {
    queue: ActionQueue,
    /// Where the pointer is, as fractions of the screen.
    pointer: (f32, f32),
    /// The stroke while the left button is held.
    stroke: Option<Stroke>,
}

struct Stroke {
    started: Instant,
    points: Vec<(f32, f32)>,
}

impl GestureInjector {
    pub fn new(queue: ActionQueue) -> Self {
        Self { queue, pointer: (0.5, 0.5), stroke: None }
    }

    fn press(&mut self) {
        self.stroke = Some(Stroke { started: Instant::now(), points: vec![self.pointer] });
    }

    fn release(&mut self) {
        let Some(mut stroke) = self.stroke.take() else { return };
        if stroke.points.last() != Some(&self.pointer) {
            stroke.points.push(self.pointer);
        }
        let ms = u32::try_from(stroke.started.elapsed().as_millis()).unwrap_or(u32::MAX).clamp(1, MAX_STROKE_MS);
        self.queue.push(Action::Stroke { points: stroke.points, ms });
    }

    fn moved(&mut self, x: i32, y: i32) {
        self.pointer = (to_fraction(x), to_fraction(y));
        let pointer = self.pointer;
        if let Some(stroke) = &mut self.stroke {
            let last = *stroke.points.last().unwrap_or(&pointer);
            let far = (pointer.0 - last.0).abs().max((pointer.1 - last.1).abs()) >= POINT_SLOP;
            if far {
                if stroke.points.len() < MAX_STROKE_POINTS {
                    stroke.points.push(pointer);
                } else if let Some(end) = stroke.points.last_mut() {
                    *end = pointer;
                }
            }
        }
    }

    fn scroll(&mut self, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let (x, y) = self.pointer;
        self.queue.push(Action::Scroll {
            x,
            y,
            dx: dx.clamp(-MAX_SCROLL, MAX_SCROLL),
            dy: dy.clamp(-MAX_SCROLL, MAX_SCROLL),
        });
    }

    fn key(&mut self, code: u32, modifiers: u8) {
        let action = match code {
            8 => Action::Edit(Edit::Backspace),
            9 => return,
            13 => Action::Edit(Edit::Enter),
            27 => Action::Global(Global::Back),
            33 => return self.scroll(0.0, -MAX_SCROLL),
            34 => return self.scroll(0.0, MAX_SCROLL),
            35 => Action::Edit(Edit::End),
            36 => Action::Edit(Edit::Home),
            37 => Action::Edit(Edit::Left),
            38 => return self.scroll(0.0, -NOTCH),
            39 => Action::Edit(Edit::Right),
            40 => return self.scroll(0.0, NOTCH),
            46 => Action::Edit(Edit::Delete),
            // The Windows/Command key alone.
            91 | 92 if modifiers & !META == 0 => Action::Global(Global::Home),
            // The context-menu key.
            93 => Action::Global(Global::Recents),
            // Browser Back and Home keys.
            166 => Action::Global(Global::Back),
            172 => Action::Global(Global::Home),
            _ => return,
        };
        self.queue.push(action);
    }

    fn character(&mut self, ch: char, modifiers: u8) {
        if modifiers & (CTRL | META) != 0 {
            let edit = match ch.to_ascii_lowercase() {
                'a' => Edit::SelectAll,
                'c' => Edit::Copy,
                'x' => Edit::Cut,
                'v' => Edit::Paste,
                _ => return,
            };
            return self.queue.push(Action::Edit(edit));
        }
        if modifiers & ALT != 0 && ch.is_ascii_alphanumeric() {
            // Alt shortcuts mean nothing on a phone; typing the letter would
            // surprise more than doing nothing.
            return;
        }
        match ch {
            '\r' | '\n' => self.queue.push(Action::Edit(Edit::Enter)),
            '\u{8}' => self.queue.push(Action::Edit(Edit::Backspace)),
            '\u{7f}' => self.queue.push(Action::Edit(Edit::Delete)),
            ch if ch.is_control() => {}
            ch => self.queue.push(Action::Text(ch.to_string())),
        }
    }
}

impl InputInjector for GestureInjector {
    fn inject(&mut self, ev: &InputEvent, _screen_w: u32, _screen_h: u32) {
        match *ev {
            InputEvent::MouseMove { x, y } => self.moved(x, y),
            InputEvent::MouseButton { button: 1, down: true } => self.press(),
            InputEvent::MouseButton { button: 1, down: false } => self.release(),
            InputEvent::MouseButton { button: 2, down: true } => self.queue.push(Action::Global(Global::Back)),
            InputEvent::MouseButton { button: 3, down: true } => self.queue.push(Action::Global(Global::Home)),
            InputEvent::MouseButton { .. } => {}
            InputEvent::Scroll { dx, dy } => self.scroll(dx as f32 * NOTCH, dy as f32 * NOTCH),
            InputEvent::Key { code, down: true, modifiers } => self.key(code, modifiers),
            InputEvent::Key { .. } => {}
            InputEvent::Char { ch, down: true, modifiers } => self.character(ch, modifiers),
            InputEvent::Char { .. } => {}
        }
    }
}

/// Protocol pointer coordinates (0..=65535) as a fraction of the screen.
fn to_fraction(coord: i32) -> f32 {
    coord.clamp(0, 65_535) as f32 / 65_535.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn injector() -> (GestureInjector, ActionQueue) {
        let queue = ActionQueue::new();
        (GestureInjector::new(queue.clone()), queue)
    }

    fn send(injector: &mut GestureInjector, events: &[InputEvent]) {
        for event in events {
            injector.inject(event, 1080, 2400);
        }
    }

    fn drain(queue: &ActionQueue) -> Vec<Action> {
        std::iter::from_fn(|| queue.next(Duration::ZERO)).collect()
    }

    const fn mv(x: i32, y: i32) -> InputEvent {
        InputEvent::MouseMove { x, y }
    }

    const fn button(button: u8, down: bool) -> InputEvent {
        InputEvent::MouseButton { button, down }
    }

    const fn ch(ch: char, modifiers: u8) -> InputEvent {
        InputEvent::Char { ch, down: true, modifiers }
    }

    const fn key(code: u32) -> InputEvent {
        InputEvent::Key { code, down: true, modifiers: 0 }
    }

    #[test]
    fn a_click_is_a_one_point_stroke_where_the_pointer_is() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[mv(32_768, 16_384), button(1, true), button(1, false)]);
        match drain(&queue).as_slice() {
            [Action::Stroke { points, ms }] => {
                assert_eq!(points.len(), 1);
                assert!((points[0].0 - 0.5).abs() < 0.001 && (points[0].1 - 0.25).abs() < 0.001);
                assert!(*ms >= 1 && *ms < 1000);
            }
            other => panic!("expected one stroke, got {other:?}"),
        }
    }

    #[test]
    fn a_drag_keeps_its_path_and_ends_where_released() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[mv(0, 0), button(1, true), mv(10_000, 0), mv(10_000, 10_000), button(1, false)]);
        let actions = drain(&queue);
        let [Action::Stroke { points, .. }] = actions.as_slice() else { panic!("{actions:?}") };
        assert_eq!(points.len(), 3);
        assert_eq!(points[0], (0.0, 0.0));
        assert!((points[2].0 - 10_000.0 / 65_535.0).abs() < 1e-6);
    }

    #[test]
    fn tiny_moves_do_not_add_points() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[mv(1000, 1000), button(1, true), mv(1001, 1001), mv(1002, 1000), button(1, false)]);
        let actions = drain(&queue);
        let [Action::Stroke { points, .. }] = actions.as_slice() else { panic!("{actions:?}") };
        // The start, and the release point (it differs from the start).
        assert!(points.len() <= 2, "{points:?}");
    }

    #[test]
    fn long_drags_are_capped_but_end_at_the_release_point() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[mv(0, 0), button(1, true)]);
        for step in 1..=1000 {
            injector.inject(&mv(step * 60, 0), 1080, 2400);
        }
        injector.inject(&button(1, false), 1080, 2400);
        let actions = drain(&queue);
        let [Action::Stroke { points, .. }] = actions.as_slice() else { panic!("{actions:?}") };
        assert!(points.len() <= MAX_STROKE_POINTS + 1, "{}", points.len());
        assert!((points.last().unwrap().0 - 60_000.0 / 65_535.0).abs() < 1e-6);
    }

    #[test]
    fn a_release_without_a_press_does_nothing() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[button(1, false)]);
        assert!(queue.is_empty());
    }

    #[test]
    fn right_and_middle_buttons_are_back_and_home() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[button(2, true), button(2, false), button(3, true), button(3, false)]);
        assert_eq!(drain(&queue), [Action::Global(Global::Back), Action::Global(Global::Home)]);
    }

    #[test]
    fn wheel_notches_scroll_at_the_pointer_and_merge() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[mv(32_768, 32_768), InputEvent::Scroll { dx: 0, dy: 1 }, InputEvent::Scroll { dx: 0, dy: 2 }]);
        let actions = drain(&queue);
        let [Action::Scroll { x, dy, dx, .. }] = actions.as_slice() else { panic!("{actions:?}") };
        assert!((x - 0.5).abs() < 0.001);
        assert!((dy - 3.0 * NOTCH).abs() < 1e-6);
        assert_eq!(*dx, 0.0);
    }

    #[test]
    fn scrolls_are_capped() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[InputEvent::Scroll { dx: 0, dy: -50 }]);
        let actions = drain(&queue);
        let [Action::Scroll { dy, .. }] = actions.as_slice() else { panic!("{actions:?}") };
        assert_eq!(*dy, -MAX_SCROLL);
    }

    #[test]
    fn typing_joins_into_one_text_action() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[ch('h', 0), ch('í', 1), ch('!', 1)]);
        assert_eq!(drain(&queue), [Action::Text("hí!".into())]);
    }

    #[test]
    fn character_releases_are_ignored() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[InputEvent::Char { ch: 'a', down: false, modifiers: 0 }]);
        assert!(queue.is_empty());
    }

    #[test]
    fn ctrl_shortcuts_become_clipboard_and_selection_edits() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[ch('a', CTRL), ch('C', CTRL | 1), ch('x', META), ch('v', CTRL), ch('z', CTRL)]);
        assert_eq!(drain(&queue), [
            Action::Edit(Edit::SelectAll),
            Action::Edit(Edit::Copy),
            Action::Edit(Edit::Cut),
            Action::Edit(Edit::Paste),
        ]);
    }

    #[test]
    fn named_keys_map_to_edits_and_buttons() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[key(8), key(13), key(27), key(46), key(37), key(39), key(36), key(35), key(91), key(93), key(9)]);
        assert_eq!(drain(&queue), [
            Action::Edit(Edit::Backspace),
            Action::Edit(Edit::Enter),
            Action::Global(Global::Back),
            Action::Edit(Edit::Delete),
            Action::Edit(Edit::Left),
            Action::Edit(Edit::Right),
            Action::Edit(Edit::Home),
            Action::Edit(Edit::End),
            Action::Global(Global::Home),
            Action::Global(Global::Recents),
        ]);
    }

    #[test]
    fn page_keys_scroll_a_page() {
        let (mut injector, queue) = injector();
        send(&mut injector, &[key(34)]);
        let actions = drain(&queue);
        let [Action::Scroll { dy, .. }] = actions.as_slice() else { panic!("{actions:?}") };
        assert_eq!(*dy, MAX_SCROLL);
    }

    #[test]
    fn a_full_queue_drops_the_oldest() {
        let queue = ActionQueue::new();
        for index in 0..MAX_QUEUED + 3 {
            // Alternate so nothing merges.
            queue.push(if index % 2 == 0 { Action::Global(Global::Back) } else { Action::Edit(Edit::Enter) });
        }
        assert_eq!(queue.len(), MAX_QUEUED);
        // Index 3 is now first: 0, 1 and 2 were dropped.
        assert_eq!(queue.next(Duration::ZERO), Some(Action::Edit(Edit::Enter)));
    }

    #[test]
    fn next_waits_for_an_action() {
        let queue = ActionQueue::new();
        let pusher = queue.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            pusher.push(Action::Global(Global::Recents));
        });
        assert_eq!(queue.next(Duration::from_secs(5)), Some(Action::Global(Global::Recents)));
        handle.join().unwrap();
        assert_eq!(queue.next(Duration::from_millis(5)), None);
    }

    #[test]
    fn actions_serialize_to_json() {
        assert_eq!(
            Action::Stroke { points: vec![(0.5, 0.25), (1.5, -1.0)], ms: 80 }.to_json(),
            r#"{"t":"stroke","ms":80,"p":[[0.5000,0.2500],[1.0000,0.0000]]}"#
        );
        assert_eq!(
            Action::Scroll { x: 0.5, y: 0.5, dx: 0.0, dy: -0.3 }.to_json(),
            r#"{"t":"scroll","x":0.5000,"y":0.5000,"dx":0.0000,"dy":-0.3000}"#
        );
        assert_eq!(Action::Text("a\"b\\\n\u{1}é".into()).to_json(), r#"{"t":"text","s":"a\"b\\\n\u0001é"}"#);
        assert_eq!(Action::Edit(Edit::SelectAll).to_json(), r#"{"t":"edit","k":"select_all"}"#);
        assert_eq!(Action::Global(Global::Notifications).to_json(), r#"{"t":"global","k":"notifications"}"#);
    }
}
