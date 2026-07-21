use std::borrow::Cow;

#[derive(Debug, Clone)]
pub(crate) enum Doc<'a> {
    Nil,
    Text(Cow<'a, str>),
    HardLine,
    SoftLine,
    SoftZero,
    Concat(Vec<Doc<'a>>),
    Indent(Box<Doc<'a>>),
    Group(Box<Doc<'a>>),
    IfBreak {
        broken: Box<Doc<'a>>,
        flat: Box<Doc<'a>>,
    },
}

impl<'a> Doc<'a> {
    pub(crate) fn text(text: impl Into<Cow<'a, str>>) -> Self {
        Self::Text(text.into())
    }

    pub(crate) fn concat(parts: impl IntoIterator<Item = Self>) -> Self {
        let mut flattened = Vec::new();
        for part in parts {
            match part {
                Self::Nil => {}
                Self::Concat(parts) => flattened.extend(parts),
                other => flattened.push(other),
            }
        }
        match flattened.len() {
            0 => Self::Nil,
            1 => flattened.pop().expect("the document has one part"),
            _ => Self::Concat(flattened),
        }
    }

    pub(crate) fn indent(self) -> Self {
        Self::Indent(Box::new(self))
    }

    pub(crate) fn group(self) -> Self {
        Self::Group(Box::new(self))
    }

    pub(crate) fn if_break(broken: Self, flat: Self) -> Self {
        Self::IfBreak {
            broken: Box::new(broken),
            flat: Box::new(flat),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Flat,
    Broken,
}

pub(crate) fn render(document: &Doc<'_>, width: usize, indent_width: usize) -> String {
    let mut renderer = Renderer {
        output: String::new(),
        width,
        indent_width,
        column: 0,
        line_start: true,
    };
    renderer.document(document, Mode::Broken, 0);
    renderer.output
}

struct Renderer {
    output: String,
    width: usize,
    indent_width: usize,
    column: usize,
    line_start: bool,
}

impl Renderer {
    fn document(&mut self, document: &Doc<'_>, mode: Mode, indentation: usize) {
        match document {
            Doc::Nil => {}
            Doc::Text(text) => self.text(text, indentation),
            Doc::HardLine => self.hardline(),
            Doc::SoftLine => match mode {
                Mode::Flat => self.text(" ", indentation),
                Mode::Broken => self.hardline(),
            },
            Doc::SoftZero => {
                if mode == Mode::Broken {
                    self.hardline();
                }
            }
            Doc::Concat(parts) => {
                for part in parts {
                    self.document(part, mode, indentation);
                }
            }
            Doc::Indent(inner) => {
                self.document(inner, mode, indentation.saturating_add(self.indent_width));
            }
            Doc::Group(inner) => {
                let inner_mode = if mode == Mode::Flat || self.fits(inner) {
                    Mode::Flat
                } else {
                    Mode::Broken
                };
                self.document(inner, inner_mode, indentation);
            }
            Doc::IfBreak { broken, flat } => match mode {
                Mode::Flat => self.document(flat, mode, indentation),
                Mode::Broken => self.document(broken, mode, indentation),
            },
        }
    }

    fn text(&mut self, text: &str, indentation: usize) {
        if text.is_empty() {
            return;
        }
        if self.line_start && !text.starts_with('\n') {
            self.output.extend(std::iter::repeat_n(' ', indentation));
            self.column = indentation;
            self.line_start = false;
        }
        self.output.push_str(text);
        if let Some(last_line) = text.rsplit_once('\n') {
            self.column = last_line.1.chars().count();
            self.line_start = last_line.1.is_empty();
        } else {
            self.column = self.column.saturating_add(text.chars().count());
            self.line_start = false;
        }
    }

    fn hardline(&mut self) {
        self.output.push('\n');
        self.column = 0;
        self.line_start = true;
    }

    fn fits(&self, document: &Doc<'_>) -> bool {
        let mut column = self.column;
        let mut stack = vec![document];
        while let Some(document) = stack.pop() {
            match document {
                Doc::Nil | Doc::SoftZero => {}
                Doc::Text(text) => {
                    if text.contains('\n') {
                        return false;
                    }
                    column = column.saturating_add(text.chars().count());
                    if column > self.width {
                        return false;
                    }
                }
                Doc::HardLine => return false,
                Doc::SoftLine => {
                    column = column.saturating_add(1);
                    if column > self.width {
                        return false;
                    }
                }
                Doc::Concat(parts) => stack.extend(parts.iter().rev()),
                Doc::Indent(inner) | Doc::Group(inner) => stack.push(inner),
                Doc::IfBreak { flat, .. } => stack.push(flat),
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(item: &str) -> Doc<'_> {
        Doc::concat([
            Doc::text("["),
            Doc::concat([Doc::SoftZero, Doc::text(item)]).indent(),
            Doc::if_break(Doc::text(","), Doc::Nil),
            Doc::SoftZero,
            Doc::text("]"),
        ])
        .group()
    }

    #[test]
    fn exact_width_flattens_and_one_more_scalar_breaks() {
        let flat = "x".repeat(98);
        assert_eq!(
            render(&list(&flat), 100, 4),
            format!("[{}]", "x".repeat(98))
        );
        let broken = "x".repeat(99);
        assert_eq!(
            render(&list(&broken), 100, 4),
            format!("[\n    {},\n]", "x".repeat(99))
        );
    }

    #[test]
    fn width_counts_unicode_scalars_and_nested_groups_flatten_preorder() {
        let inner = Doc::concat([
            Doc::text("["),
            Doc::SoftZero,
            Doc::text("é"),
            Doc::SoftZero,
            Doc::text("]"),
        ])
        .group();
        let prefix = "x".repeat(98);
        let outer = Doc::concat([Doc::text(&prefix), Doc::SoftLine, inner]).group();
        assert_eq!(render(&outer, 100, 4), format!("{}\n[é]", "x".repeat(98)));
    }

    #[test]
    fn hardlines_indent_lazily_without_trailing_spaces() {
        let document = Doc::concat([
            Doc::text("{"),
            Doc::concat([Doc::HardLine, Doc::text("value"), Doc::HardLine]).indent(),
            Doc::text("}"),
        ]);
        assert_eq!(render(&document, 100, 4), "{\n    value\n}");
    }
}
