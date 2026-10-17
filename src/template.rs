#[derive(Clone, Debug, PartialEq)]
enum Part {
  Literal(String),
  Name,
  Title,
}

/// An extremely simple templating engine that caches templates and only has the
/// variables "name" and "title". '{{' and '}}' are used to insert literal
/// braces. Although the escape for '}' is redundant, it's implemented this way
/// for consistency.
#[derive(Clone, Debug, PartialEq)]
pub struct TitleTemplate(Vec<Part>);

impl TitleTemplate {
  /// Create a new template
  pub fn parse(template: &str) -> Result<Self, String> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
      /// Not lexing a variable
      Base,

      /// Encountered a single '{'
      Open,

      /// Lexing a variable
      Variable,

      /// Encountered a single '}'
      Close,
    }

    let mut parts = Vec::new();
    let mut state = State::Base;

    // buffer to store current run of literal characters
    let mut literal = String::new();

    // buffer to store variable name as read in the template
    let mut var = String::with_capacity(5); // length of "title"

    for c in template.chars() {
      match (state, c) {
        (State::Base, '{') => {
          state = State::Open;
        }

        (State::Base, '}') => {
          state = State::Close;
        }

        (State::Base, c) => {
          literal.push(c);
        }

        (State::Open, '{') => {
          literal.push('{');
          state = State::Base;
        }

        (State::Open, c) => {
          parts.push(Part::Literal(std::mem::take(&mut literal)));
          var.push(c);
          state = State::Variable;
        }

        (State::Variable, '}') => {
          let trimmed = var.trim();
          if trimmed.eq_ignore_ascii_case("name") {
            parts.push(Part::Name);
          }
          else if trimmed.eq_ignore_ascii_case("title") {
            parts.push(Part::Title);
          }
          else {
            return Err(format!("unknown variable: '{var}'"));
          }

          var.clear();
          state = State::Base;
        }

        (State::Variable, c) => {
          var.push(c);
        }

        (State::Close, '}') => {
          literal.push('}');
          state = State::Base; // can only enter this state from base state
        }

        // a single '}' while in base state is illegal, must be escaped as '}}'
        (State::Close, _) => return Err("unmatched '}'".to_string()),
      }
    }

    if state == State::Open {
      return Err("unmatched '{'".to_string());
    }

    if state == State::Variable {
      return Err(format!("unclosed variable: '{var}'"));
    }

    if state == State::Close {
      return Err("unmatched '}'".to_string());
    }

    if !literal.is_empty() {
      parts.push(Part::Literal(literal));
    }

    Ok(Self(parts))
  }

  /// Replace variables in the template with their real values
  pub fn replace(&self, name: &str, title: &str) -> String {
    let mut out = String::new();

    for part in &self.0 {
      match part {
        Part::Literal(s) => out.push_str(s),
        Part::Name => out.push_str(name),
        Part::Title => out.push_str(title),
      }
    }

    out
  }
}

const DEFAULT_TEMPLATE: &str = "{name} | {title}";

impl Default for TitleTemplate {
  fn default() -> Self {
    // this is safe according to the `default_parses` test below
    Self::parse(DEFAULT_TEMPLATE).unwrap()
  }
}

#[test]
fn default() {
  let template = TitleTemplate::parse(DEFAULT_TEMPLATE).expect("The default template should parse");
  assert_eq!(
    template.replace("Firefox", "crates.io: Rust Package Registry"),
    "Firefox | crates.io: Rust Package Registry"
  );
}

#[test]
fn basic() {
  assert_eq!(
    TitleTemplate::parse("{title}")
      .unwrap()
      .replace("Firefox", "crates.io: Rust Package Registry"),
    "crates.io: Rust Package Registry"
  );

  assert_eq!(
    TitleTemplate::parse("{title} | {name}")
      .unwrap()
      .replace("Firefox", "crates.io: Rust Package Registry"),
    "crates.io: Rust Package Registry | Firefox"
  );

  assert_eq!(
    TitleTemplate::parse("{name}{title}")
      .unwrap()
      .replace("a", "b"),
    "ab"
  );

  assert_eq!(
    TitleTemplate::parse("{name} {name}")
      .unwrap()
      .replace("Firefox", "literally anything"),
    "Firefox Firefox"
  );

  assert_eq!(
    TitleTemplate::parse("[{name}] {title}")
      .unwrap()
      .replace("Firefox", "crates.io: Rust Package Registry"),
    "[Firefox] crates.io: Rust Package Registry"
  );

  assert_eq!(
    TitleTemplate::parse("  {name}  {title}  ")
      .unwrap()
      .replace("Firefox", "crates.io: Rust Package Registry"),
    "  Firefox  crates.io: Rust Package Registry  "
  );
}

#[test]
fn escapes() {
  assert_eq!(
    TitleTemplate::parse("{{{name}}} {title}")
      .unwrap()
      .replace("Firefox", "crates.io: Rust Package Registry"),
    "{Firefox} crates.io: Rust Package Registry"
  );

  assert_eq!(TitleTemplate::parse("{{").unwrap().replace("A", "B"), "{");
  assert_eq!(TitleTemplate::parse("}}").unwrap().replace("A", "B"), "}");

  assert_eq!(
    TitleTemplate::parse("{{}}").unwrap().replace("A", "B"),
    "{}"
  );

  assert_eq!(
    TitleTemplate::parse("{{name}}").unwrap().replace("A", "B"),
    "{name}"
  );

  assert_eq!(
    TitleTemplate::parse("{{{{}}}}").unwrap().replace("A", "B"),
    "{{}}"
  );
}

#[test]
fn inner_whitespace() {
  for (i, template) in [
    "{ name } | { title}",
    "{name } | { title }",
    "{name} | {title }",
    "{name} | { title }",
  ]
  .iter()
  .enumerate()
  {
    assert_eq!(
      TitleTemplate::parse(template)
        .unwrap_or_else(|why| panic!("failed to parse test {i}: {why}"))
        .replace("Firefox", "crates.io: Rust Package Registry"),
      "Firefox | crates.io: Rust Package Registry"
    );
  }
}

#[test]
fn empty() {
  assert_eq!(
    TitleTemplate::parse("").unwrap().replace("App", "title"),
    ""
  );

  assert_eq!(
    TitleTemplate::parse("{name}").unwrap().replace("", "title"),
    ""
  );

  assert_eq!(
    TitleTemplate::parse("{title}").unwrap().replace("App", ""),
    ""
  );

  assert_eq!(
    TitleTemplate::parse("  ").unwrap().replace("App", "title"),
    "  "
  );
}

#[test]
fn case_insensitive() {
  assert_eq!(
    TitleTemplate::parse("{NAME} | {TITLE}")
      .unwrap()
      .replace("Firefox", "crates.io: Rust Package Registry"),
    "Firefox | crates.io: Rust Package Registry"
  );

  assert_eq!(
    TitleTemplate::parse("{Name} | {titlE}")
      .unwrap()
      .replace("Firefox", "crates.io: Rust Package Registry"),
    "Firefox | crates.io: Rust Package Registry"
  );
}

#[test]
fn errors() {
  assert_eq!(
    TitleTemplate::parse("{app_id} | {title}").unwrap_err(),
    "unknown variable: 'app_id'"
  );

  assert_eq!(
    TitleTemplate::parse("{name} {").unwrap_err(),
    "unmatched '{'"
  );
  assert_eq!(
    TitleTemplate::parse("{name} | {title").unwrap_err(),
    "unclosed variable: 'title'"
  );

  assert_eq!(
    TitleTemplate::parse("{name} | title}").unwrap_err(),
    "unmatched '}'"
  );
  assert_eq!(
    TitleTemplate::parse("{name} } {title}").unwrap_err(),
    "unmatched '}'"
  );
}
