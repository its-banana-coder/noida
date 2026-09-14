use super::*;
use syntect::highlighting::Theme;

fn syntax() -> Syntax {
    Syntax { set: SyntaxSet::load_defaults_newlines(), dark: Theme::default(), light: Theme::default() }
}

fn doc(name: &str, text: &str) -> Doc {
    Doc::from_text(Path::new(name), text, &syntax())
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn typ(d: &mut Doc, s: &str) {
    for c in s.chars() {
        d.handle_key(key(KeyCode::Char(c)));
    }
}

#[test]
fn edit_and_undo() {
    let mut d = doc("a.rs", "fn main() {\n    let x = 1;\n}\n");
    d.goto(2, None, None);
    assert_eq!(d.primary().head, (1, 4));
    d.handle_key(key(KeyCode::End));
    d.handle_key(key(KeyCode::Enter));
    typ(&mut d, "y");
    assert_eq!(d.lines[2], "    y");
    d.handle_key(ctrl('z'));
    d.handle_key(ctrl('z'));
    assert_eq!(d.lines.len(), 3);
    d.goto(2, Some(5), None);
    d.insert_text("a\nb");
    assert_eq!(d.lines[1], "    a");
    assert_eq!(d.lines[2], "blet x = 1;");
}

#[test]
fn auto_close_and_newline_between_braces() {
    let mut d = doc("a.rs", "\n");
    typ(&mut d, "fn f(");
    assert_eq!(d.lines[0], "fn f()");
    typ(&mut d, ")");
    assert_eq!(d.lines[0], "fn f()");
    typ(&mut d, " {");
    d.handle_key(key(KeyCode::Enter));
    assert_eq!(d.text(), "fn f() {\n    \n}");
    assert_eq!(d.primary().head, (1, 4));
    // Backspace inside an empty pair removes both.
    let mut d = doc("a.rs", "\n");
    typ(&mut d, "[");
    d.handle_key(key(KeyCode::Backspace));
    assert_eq!(d.lines[0], "");
    // Quotes don't pair after a word character.
    typ(&mut d, "don't");
    assert_eq!(d.lines[0], "don't");
}

#[test]
fn wraps_selection_and_closes_tags() {
    let mut d = doc("a.rs", "value\n");
    d.select_all();
    typ(&mut d, "(");
    assert_eq!(d.lines[0], "(value)");
    assert_eq!(d.selected_text().as_deref(), Some("value"));
    let mut d = doc("x.tsx", "\n");
    typ(&mut d, "<div className=\"a\">");
    assert_eq!(d.lines[0], "<div className=\"a\"></div>");
    assert_eq!(d.primary().head, (0, 19));
}

#[test]
fn multi_cursor_typing_and_ctrl_d() {
    let mut d = doc("a.rs", "let foo = 1;\nlet foo2 = foo;\n");
    d.goto(1, Some(5), None);
    d.handle_key(ctrl('d'));
    assert_eq!(d.selected_text().as_deref(), Some("foo"));
    d.handle_key(ctrl('d'));
    d.handle_key(ctrl('d'));
    assert_eq!(d.cursor_count(), 3);
    typ(&mut d, "bar");
    assert_eq!(d.text(), "let bar = 1;\nlet bar2 = bar;");
    // Cursors stay put across a newline edit on an earlier line.
    d.handle_key(key(KeyCode::Esc));
    assert_eq!(d.cursor_count(), 1);
}

#[test]
fn add_cursor_below_and_paste_distributes() {
    let mut d = doc("a.txt", "a\nb\nc\n");
    d.goto(1, Some(2), None);
    d.add_cursor_vertical(true);
    d.add_cursor_vertical(true);
    assert_eq!(d.cursor_count(), 3);
    d.insert_text("1\n2\n3");
    assert_eq!(d.text(), "a1\nb2\nc3");
    d.handle_key(key(KeyCode::Backspace));
    assert_eq!(d.text(), "a\nb\nc");
}

#[test]
fn comments_move_duplicate_delete() {
    let mut d = doc("a.py", "def f():\n    return 1\n");
    d.select_all();
    d.handle_key(ctrl('/'));
    assert_eq!(d.text(), "# def f():\n#     return 1");
    d.handle_key(ctrl('/'));
    assert_eq!(d.text(), "def f():\n    return 1");

    let mut d = doc("a.txt", "one\ntwo\nthree\n");
    d.goto(1, None, None);
    d.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT));
    assert_eq!(d.text(), "two\none\nthree");
    assert_eq!(d.primary().head.0, 1);
    d.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT | KeyModifiers::SHIFT));
    assert_eq!(d.text(), "two\none\none\nthree");
    assert_eq!(d.primary().head.0, 2);
    d.handle_key(ctrl('k'));
    assert_eq!(d.text(), "two\none\nthree");
}

#[test]
fn search_and_replace() {
    let mut d = doc("a.txt", "foo Foo foo\nbar foo\n");
    d.set_search("foo", SearchOpts::default());
    assert_eq!(d.search.as_ref().unwrap().matches.len(), 4);
    d.set_search("foo", SearchOpts { case: true, ..Default::default() });
    assert_eq!(d.search.as_ref().unwrap().matches.len(), 3);
    d.set_search("fo+", SearchOpts { regex: true, word: true, case: true, ..Default::default() });
    assert_eq!(d.search.as_ref().unwrap().matches.len(), 3);
    assert!(d.replace_current("X"));
    assert_eq!(d.lines[0], "X Foo foo");
    assert_eq!(d.replace_all("Y"), 2);
    assert_eq!(d.text(), "X Foo Y\nbar Y");
    d.handle_key(ctrl('z'));
    assert_eq!(d.text(), "X Foo foo\nbar foo");
    d.set_search("(\\w+) foo", SearchOpts { regex: true, case: true, ..Default::default() });
    d.replace_all("$1-baz");
    assert_eq!(d.text(), "X Foo-baz\nbar-baz");
}

#[test]
fn search_in_selection() {
    let mut d = doc("a.txt", "a a\na a\na a\n");
    d.goto(2, Some(1), None);
    d.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::SHIFT));
    d.set_search("a", SearchOpts { in_selection: true, ..Default::default() });
    assert_eq!(d.search.as_ref().unwrap().matches.len(), 2);
    assert_eq!(d.select_all_matches(), 2);
}

#[test]
fn folding_hides_lines() {
    let mut d = doc("a.py", "def f():\n    a = 1\n    b = 2\nx = 3\n");
    assert_eq!(d.fold_range(0), Some((0, 2)));
    assert!(d.toggle_fold(1));
    d.update_hidden();
    assert!(d.is_hidden(1) && d.is_hidden(2) && !d.is_hidden(3));
    d.goto(1, Some(1), None);
    d.handle_key(key(KeyCode::Down));
    assert_eq!(d.primary().head.0, 3);
    d.goto(2, None, None);
    d.update_hidden();
    assert!(!d.is_hidden(1), "jumping into a fold opens it");
}

#[test]
fn wrapped_vertical_movement() {
    let mut d = doc("a.txt", &format!("{}\nend\n", "x".repeat(25)));
    d.wrap = true;
    d.view.width = 10;
    d.view.height = 10;
    assert_eq!(d.segments(0), vec![0, 10, 20]);
    d.goto(1, Some(3), None);
    d.handle_key(key(KeyCode::Down));
    assert_eq!(d.primary().head, (0, 12));
    d.handle_key(key(KeyCode::Down));
    d.handle_key(key(KeyCode::Down));
    assert_eq!(d.primary().head.0, 1);
}

#[test]
fn lsp_edits_apply_in_utf16() {
    let mut d = doc("a.rs", "let é = old;\n");
    d.apply_lsp_edits(vec![((0, 8), (0, 11), "new".into())]);
    assert_eq!(d.lines[0], "let é = new;");
}
