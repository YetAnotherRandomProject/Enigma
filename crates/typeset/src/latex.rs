//! Conversion des formules LaTeX (syntaxe d'Obsidian, `$…$`) vers la
//! syntaxe mathématique de Typst. Couvre l'usage courant : fractions,
//! racines, indices et exposants, lettres grecques, opérateurs, flèches,
//! ensembles, `\text`, `\mathbb`, accents, `\left( … \right)`.
//! Une commande inconnue est reprise sans la barre oblique (beaucoup de
//! noms sont communs aux deux, `\sin` → `sin`).

/// Commandes simples : LaTeX → Typst.
const SYMBOLS: &[(&str, &str)] = &[
    ("infty", "infinity"),
    ("le", "<="),
    ("leq", "<="),
    ("ge", ">="),
    ("geq", ">="),
    ("ne", "!="),
    ("neq", "!="),
    ("pm", "plus.minus"),
    ("mp", "minus.plus"),
    ("times", "times"),
    ("cdot", "dot.op"),
    ("div", "div"),
    ("to", "->"),
    ("rightarrow", "->"),
    ("leftarrow", "<-"),
    ("gets", "<-"),
    ("leftrightarrow", "<->"),
    ("Rightarrow", "=>"),
    ("implies", "=>"),
    ("Leftarrow", "arrow.l.double"),
    ("Leftrightarrow", "<=>"),
    ("iff", "<=>"),
    ("mapsto", "|->"),
    ("approx", "approx"),
    ("equiv", "equiv"),
    ("sim", "tilde.op"),
    ("simeq", "tilde.eq"),
    ("propto", "prop"),
    ("in", "in"),
    ("notin", "in.not"),
    ("ni", "in.rev"),
    ("subset", "subset"),
    ("subseteq", "subset.eq"),
    ("supset", "supset"),
    ("supseteq", "supset.eq"),
    ("cup", "union"),
    ("cap", "sect"),
    ("setminus", "without"),
    ("emptyset", "emptyset"),
    ("varnothing", "emptyset"),
    ("forall", "forall"),
    ("exists", "exists"),
    ("neg", "not"),
    ("lnot", "not"),
    ("land", "and"),
    ("wedge", "and"),
    ("lor", "or"),
    ("vee", "or"),
    ("partial", "diff"),
    ("nabla", "nabla"),
    ("sum", "sum"),
    ("prod", "product"),
    ("int", "integral"),
    ("iint", "integral.double"),
    ("oint", "integral.cont"),
    ("ldots", "dots.h"),
    ("dots", "dots.h"),
    ("cdots", "dots.c"),
    ("vdots", "dots.v"),
    ("ddots", "dots.down"),
    ("quad", "quad"),
    ("qquad", "wide"),
    ("circ", "compose"),
    ("degree", "degree"),
    ("angle", "angle"),
    ("perp", "perp"),
    ("parallel", "parallel"),
    ("mid", "divides"),
    ("ell", "ell"),
    ("hbar", "planck.reduce"),
    ("langle", "angle.l"),
    ("rangle", "angle.r"),
    ("lfloor", "floor.l"),
    ("rfloor", "floor.r"),
    ("lceil", "ceil.l"),
    ("rceil", "ceil.r"),
    ("star", "star.op"),
    ("ast", "*"),
    ("prime", "prime"),
    ("varepsilon", "epsilon"),
    ("epsilon", "epsilon.alt"),
    ("varphi", "phi"),
    ("phi", "phi.alt"),
    ("vartheta", "theta.alt"),
];

const GREEK: &[&str] = &[
    "alpha", "beta", "gamma", "delta", "zeta", "eta", "theta", "iota", "kappa", "lambda", "mu",
    "nu", "xi", "omicron", "pi", "rho", "sigma", "tau", "upsilon", "chi", "psi", "omega", "Gamma",
    "Delta", "Theta", "Lambda", "Xi", "Pi", "Sigma", "Upsilon", "Phi", "Psi", "Omega",
];

/// Fonctions à un argument : LaTeX → fonction Typst.
const UNARY: &[(&str, &str)] = &[
    ("sqrt", "sqrt"),
    ("mathbf", "bold"),
    ("boldsymbol", "bold"),
    ("mathit", "italic"),
    ("mathcal", "cal"),
    ("mathfrak", "frak"),
    ("mathsf", "sans"),
    ("mathtt", "mono"),
    ("overline", "overline"),
    ("underline", "underline"),
    ("bar", "macron"),
    ("hat", "hat"),
    ("widehat", "hat"),
    ("tilde", "tilde"),
    ("widetilde", "tilde"),
    ("vec", "arrow"),
    ("dot", "dot"),
    ("ddot", "dot.double"),
    ("overbrace", "overbrace"),
    ("underbrace", "underbrace"),
    ("abs", "abs"),
    ("norm", "norm"),
];

struct P<'a> {
    s: &'a [char],
    i: usize,
}

impl P<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.i += 1;
        }
    }

    fn command(&mut self) -> String {
        // Après `\`.
        let start = self.i;
        if self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                self.i += 1;
            }
        } else if self.peek().is_some() {
            self.i += 1;
        }
        self.s[start..self.i].iter().collect()
    }

    /// Texte brut d'un groupe `{…}` (pour `\text`).
    fn raw_group(&mut self) -> String {
        self.skip_ws();
        if self.peek() != Some('{') {
            return self
                .peek()
                .map(|c| {
                    self.i += 1;
                    c.to_string()
                })
                .unwrap_or_default();
        }
        self.i += 1;
        let mut depth = 1;
        let mut out = String::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            out.push(c);
        }
        out
    }

    /// Un argument : groupe `{…}` converti, ou un seul élément.
    fn arg(&mut self) -> String {
        self.skip_ws();
        match self.peek() {
            Some('{') => {
                self.i += 1;
                let inner = self.seq(Some('}'));
                self.i += 1;
                inner
            }
            Some('\\') => {
                self.i += 1;
                self.command_out()
            }
            Some(c) => {
                self.i += 1;
                c.to_string()
            }
            None => String::new(),
        }
    }

    /// Argument optionnel `[…]`.
    fn opt_arg(&mut self) -> Option<String> {
        self.skip_ws();
        if self.peek() != Some('[') {
            return None;
        }
        self.i += 1;
        let inner = self.seq(Some(']'));
        self.i += 1;
        Some(inner)
    }

    fn command_out(&mut self) -> String {
        let name = self.command();
        match name.as_str() {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let a = self.arg();
                let b = self.arg();
                format!("frac({a}, {b})")
            }
            "binom" => {
                let a = self.arg();
                let b = self.arg();
                format!("binom({a}, {b})")
            }
            "sqrt" => match self.opt_arg() {
                Some(n) => {
                    let x = self.arg();
                    format!("root({n}, {x})")
                }
                None => format!("sqrt({})", self.arg()),
            },
            "text" | "textrm" | "mathrm" | "textit" | "textbf" | "operatorname" => {
                let t = self.raw_group().replace('"', "\\\"");
                format!("\"{t}\"")
            }
            "mathbb" => {
                let t = self.raw_group();
                t.chars()
                    .map(|c| format!("{c}{c}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "displaystyle" | "limits" => {
                // Typst ajuste lui-même la taille des délimiteurs.
                self.skip_ws();
                match self.peek() {
                    Some('.') => {
                        self.i += 1;
                        String::new()
                    }
                    Some('\\') if name == "left" || name == "right" => {
                        self.i += 1;
                        self.command_out()
                    }
                    _ => String::new(),
                }
            }
            "{" => "{".to_owned(),
            "}" => "}".to_owned(),
            "%" => "%".to_owned(),
            "#" => "\\#".to_owned(),
            "_" => "\\_".to_owned(),
            "&" => "\\&".to_owned(),
            "$" => "\\$".to_owned(),
            "\\" => "\\ ".to_owned(),
            "," => "thin".to_owned(),
            ";" | ":" => "med".to_owned(),
            "!" => String::new(),
            " " => "space".to_owned(),
            "|" => "||".to_owned(),
            n => {
                if let Some((_, t)) = SYMBOLS.iter().find(|(l, _)| *l == n) {
                    (*t).to_owned()
                } else if GREEK.contains(&n) {
                    n.to_owned()
                } else if let Some((_, f)) = UNARY.iter().find(|(l, _)| *l == n) {
                    format!("{f}({})", self.arg())
                } else {
                    n.to_owned()
                }
            }
        }
    }

    /// Suite d'éléments jusqu'à `end` (non consommé) ou la fin.
    fn seq(&mut self, end: Option<char>) -> String {
        let mut out = String::new();
        let mut last_letter = false;
        let push = |out: &mut String, piece: &str, letter: bool, last: &mut bool| {
            if piece.is_empty() {
                return;
            }
            // Séparer les éléments (deux lettres collées formeraient un nom
            // de variable en Typst).
            if !out.is_empty()
                && !out.ends_with([' ', '(', '^', '_'])
                && !piece.starts_with([')', ',', ']'])
            {
                out.push(' ');
            }
            out.push_str(piece);
            *last = letter;
        };
        while let Some(c) = self.peek() {
            if Some(c) == end {
                break;
            }
            match c {
                '\\' => {
                    self.i += 1;
                    let piece = self.command_out();
                    push(&mut out, &piece, false, &mut last_letter);
                }
                '{' => {
                    self.i += 1;
                    let inner = self.seq(Some('}'));
                    self.i += 1;
                    push(&mut out, &inner, false, &mut last_letter);
                }
                '^' | '_' => {
                    self.i += 1;
                    let a = self.arg();
                    let simple = a.chars().count() == 1 || a.chars().all(|c| c.is_ascii_digit());
                    let script = if simple { a } else { format!("({a})") };
                    out.push(c);
                    out.push_str(&script);
                    last_letter = false;
                }
                c if c.is_whitespace() => self.i += 1,
                c if c.is_ascii_digit() || c == '.' => {
                    let start = self.i;
                    while self.peek().is_some_and(|d| d.is_ascii_digit() || d == '.') {
                        self.i += 1;
                    }
                    let n: String = self.s[start..self.i].iter().collect();
                    push(&mut out, &n, false, &mut last_letter);
                }
                '"' => {
                    self.i += 1;
                    push(&mut out, "\\\"", false, &mut last_letter);
                }
                c => {
                    self.i += 1;
                    push(
                        &mut out,
                        &c.to_string(),
                        c.is_alphabetic(),
                        &mut last_letter,
                    );
                }
            }
        }
        out
    }
}

/// Formule LaTeX → formule Typst (sans les `$`).
pub fn latex_to_typst(latex: &str) -> String {
    let chars: Vec<char> = latex.chars().collect();
    let mut p = P { s: &chars, i: 0 };
    let mut out = String::new();
    // Tolère des accolades fermantes en trop.
    loop {
        out.push_str(&p.seq(Some('}')));
        if p.peek().is_none() {
            break;
        }
        p.i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_common_latex() {
        assert_eq!(latex_to_typst(r"\frac{a+b}{2}"), "frac(a + b, 2)");
        assert_eq!(latex_to_typst(r"x^2 + y_{i+1}"), "x^2 + y_(i + 1)");
        assert_eq!(latex_to_typst(r"\sqrt[3]{x}"), "root(3, x)");
        assert_eq!(latex_to_typst(r"\alpha \le \beta"), "alpha <= beta");
        assert_eq!(latex_to_typst("ab"), "a b", "deux variables");
        assert_eq!(latex_to_typst(r"\text{si } x"), "\"si \" x");
        assert_eq!(latex_to_typst(r"\mathbb{R}"), "RR");
        assert_eq!(latex_to_typst(r"\left( x \right)"), "(x)");
        assert_eq!(latex_to_typst(r"\sum_{i=1}^{n} i"), "sum_(i = 1)^n i");
        assert_eq!(latex_to_typst(r"1945 \times 2"), "1945 times 2");
        assert_eq!(latex_to_typst(r"\vec{v}"), "arrow(v)");
    }
}
