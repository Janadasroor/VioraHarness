pub(crate) fn latex_symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" => "ε",
        "varepsilon" => "ɛ",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "omicron" => "ο",
        "pi" => "π",
        "varpi" => "ϖ",
        "rho" => "ρ",
        "varrho" => "ϱ",
        "sigma" => "σ",
        "varsigma" => "ς",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" => "φ",
        "varphi" => "ϕ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        "infty" => "∞",
        "leq" => "≤",
        "le" => "≤",
        "geq" => "≥",
        "ge" => "≥",
        "neq" => "≠",
        "ne" => "≠",
        "approx" => "≈",
        "equiv" => "≡",
        "times" => "×",
        "cdot" => "·",
        "pm" => "±",
        "mp" => "∓",
        "div" => "÷",
        "sum" => "∑",
        "int" => "∫",
        "oint" => "∮",
        "prod" => "∏",
        "coprod" => "∐",
        "sqrt" => "√",
        "partial" => "∂",
        "nabla" => "∇",
        "in" => "∈",
        "notin" => "∉",
        "ni" => "∋",
        "subset" => "⊂",
        "subseteq" => "⊆",
        "supset" => "⊃",
        "supseteq" => "⊇",
        "cup" => "∪",
        "cap" => "∩",
        "to" => "→",
        "rightarrow" => "→",
        "leftarrow" => "←",
        "gets" => "←",
        "leftrightarrow" => "↔",
        "Rightarrow" => "⇒",
        "Leftarrow" => "⇐",
        "implies" => "⟹",
        "ldots" => "…",
        "cdots" => "⋯",
        "vdots" => "⋮",
        "ddots" => "⋱",
        "forall" => "∀",
        "exists" => "∃",
        "neg" => "¬",
        "lnot" => "¬",
        "land" => "∧",
        "lor" => "∨",
        "top" => "⊤",
        "bot" => "⊥",
        "angle" => "∠",
        "perp" => "⊥",
        "parallel" => "∥",
        "sim" => "∼",
        "cong" => "≅",
        "propto" => "∝",
        "hbar" => "ħ",
        "ell" => "ℓ",
        "Re" => "ℜ",
        "Im" => "ℑ",
        "aleph" => "ℵ",
        "emptyset" => "∅",
        "circ" => "∘",
        "bullet" => "•",
        "degree" => "°",
        "prime" => "′",

        "quad" => "  ",
        "qquad" => "    ",
        _ => return None,
    })
}

pub(crate) fn blackboard(ch: char) -> Option<char> {
    Some(match ch {
        'R' => 'ℝ',
        'N' => 'ℕ',
        'Z' => 'ℤ',
        'Q' => 'ℚ',
        'C' => 'ℂ',
        'H' => 'ℍ',
        'P' => 'ℙ',
        'E' => '𝔼',
        'F' => '𝔽',
        _ => return None,
    })
}

pub(crate) fn translate_latex(s: &str) -> String {
    pub(crate) fn braced(chars: &[char], i: &mut usize) -> Option<String> {
        if chars.get(*i) != Some(&'{') {
            return None;
        }
        *i += 1;
        let mut depth = 1;
        let start = *i;
        while *i < chars.len() {
            match chars[*i] {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let inner: String = chars[start..*i].iter().collect();
                        *i += 1;
                        return Some(translate_latex(&inner));
                    }
                }
                _ => {}
            }
            *i += 1;
        }
        None
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '{' {
            if let Some(inner) = braced(&chars, &mut i) {
                out.push_str(&inner);
            } else {
                break;
            }
            continue;
        }
        if c == '}' {
            i += 1;
            continue;
        }
        if c != '\\' {
            out.push(c);
            i += 1;
            continue;
        }

        i += 1;
        let Some(&n) = chars.get(i) else {
            out.push('\\');
            break;
        };
        if !n.is_ascii_alphabetic() {
            match n {
                '\\' => out.push('\n'),

                ' ' | ',' | ';' | ':' => out.push(' '),

                '!' => {}

                other => out.push(other),
            }
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_alphabetic() {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect();
        if name == "left" || name == "right" {
            continue;
        }
        if name == "frac" {
            let a = braced(&chars, &mut i);
            let b = braced(&chars, &mut i);
            match (a, b) {
                (Some(a), Some(b)) => {
                    out.push_str(&format!("({a})/({b})"));
                }
                _ => out.push_str("frac"),
            }
            continue;
        }
        if name == "binom" || name == "dbinom" {
            let a = braced(&chars, &mut i);
            let b = braced(&chars, &mut i);
            match (a, b) {
                (Some(a), Some(b)) => {
                    out.push_str(&format!("({a} choose {b})"));
                }
                _ => out.push_str(&name),
            }
            continue;
        }
        if name == "sqrt" {
            if let Some(x) = braced(&chars, &mut i) {
                out.push_str(&format!("√({x})"));
            } else {
                out.push('√');
            }
            continue;
        }
        if name == "text" || name == "mathrm" || name == "mathbf" || name == "mathit" {
            if let Some(x) = braced(&chars, &mut i) {
                out.push_str(&x);
            }
            continue;
        }
        if name == "mathbb" {
            if let Some(x) = braced(&chars, &mut i) {
                let mapped: String = x.chars().map(|ch| blackboard(ch).unwrap_or(ch)).collect();
                out.push_str(&mapped);
            }
            continue;
        }
        if name == "hspace" {
            let _ = braced(&chars, &mut i);
            out.push(' ');
            continue;
        }
        if name == "vspace" {
            let _ = braced(&chars, &mut i);
            out.push('\n');
            continue;
        }
        if let Some(sym) = latex_symbol(&name) {
            out.push_str(sym);
            continue;
        }

        if let Some(x) = braced(&chars, &mut i) {
            out.push_str(&x);
        } else {
            out.push_str(&name);
        }
    }
    out
}

/// Inline math stays on one line (web inline style): use the 2D layout
/// only when it collapses to a single row, otherwise keep the flat
/// legacy translation.
pub(crate) fn render_inline_math(inner: &str) -> String {
    let canvas = layout_math(inner, false);
    if canvas.height() == 1 {
        let line = canvas.rows.into_iter().next().unwrap_or_default();
        let t = line.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    translate_latex(inner)
}

fn take_braced_arg(chars: &[char], mut j: usize) -> Option<(String, usize)> {
    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    if chars.get(j) != Some(&'{') {
        return None;
    }
    j += 1;
    let mut depth = 1;
    let start = j;
    while j < chars.len() {
        match chars[j] {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((chars[start..j].iter().collect(), j + 1));
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// 2D math canvas: rows padded to equal display width, plus the baseline
/// row index that horizontal composition aligns on. This is the character
/// grid analogue of the web box model (boxes with height/depth metrics).
#[derive(Clone, Debug)]
pub(crate) struct Canvas {
    rows: Vec<String>,
    baseline: usize,
    /// Came whole from one braced group: behaves as an ordinary atom in
    /// outer spacing (TeX groups are Ord).
    grouped: bool,
}

fn display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr as _;
    s.width()
}

fn pad_to_width(s: &str, w: usize) -> String {
    let cur = display_width(s);
    if cur >= w {
        return s.to_string();
    }
    format!("{s}{}", " ".repeat(w - cur))
}

/// Atom class for TeX-like spacing: 0 ordinary, 1 binary, 2 relation,
/// 3 opening, 4 closing. Anything else (words, numbers, tall boxes) is
/// ordinary.
fn atom_class(row: &str) -> u8 {
    let mut chars = row.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => match c {
            '+' | '-' | '−' | '×' | '·' | '⋅' | '±' | '∓' | '÷' | '∗' | '∩' | '∪' | '∧' | '∨'
            | '⊕' | '⊗' | '⊎' | '∘' | '•' => 1,
            '=' | '≠' | '<' | '>' | '≤' | '≥' | '≈' | '≡' | '∼' | '≅' | '∝' | '→' | '←' | '↔'
            | '⇒' | '⇐' | '⇔' | '⟹' | '∈' | '∋' | '⊂' | '⊃' | '⊆' | '⊇' | '⊥' | '∥' => {
                2
            }
            '(' | '[' | '{' | '⟨' | '⌈' | '⌊' | '|' | '‖' => 3,
            ')' | ']' | '}' | '⟩' | '⌉' | '⌋' => 4,
            _ => 0,
        },
        _ => 0,
    }
}

fn part_class(c: &Canvas) -> u8 {
    if c.grouped || c.height() != 1 {
        return 0;
    }
    atom_class(c.rows.first().map(String::as_str).unwrap_or(""))
}

/// Join atoms with single spaces around binary/relation operators
/// (a binary operator stays tight when unary: at the start or right
/// after another operator, a relation, or an opening delimiter).
fn join_atoms(parts: Vec<Canvas>) -> Canvas {
    let mut spaced: Vec<Canvas> = Vec::new();
    let mut prev2: Option<u8> = None;
    let mut prev: Option<u8> = None;
    for p in parts {
        let rc = part_class(&p);
        let need = match (prev, rc) {
            (None, _) => false,
            (_, 4) => false,
            (Some(3), _) => false,
            (_, 2) => true,
            (Some(2), _) => true,
            (_, 1) => !matches!(prev, Some(1) | Some(2) | Some(3)),
            (Some(1), _) => !matches!(prev2, None | Some(1) | Some(2) | Some(3)),
            _ => false,
        };
        if need {
            spaced.push(Canvas::blank(1, 1));
        }
        spaced.push(p);
        prev2 = prev;
        prev = Some(rc);
    }
    Canvas::hcat(spaced)
}

impl Canvas {
    fn empty() -> Self {
        Self {
            rows: Vec::new(),
            baseline: 0,
            grouped: false,
        }
    }

    fn blank(width: usize, height: usize) -> Self {
        Self {
            rows: vec![" ".repeat(width); height],
            baseline: 0,
            grouped: false,
        }
    }

    fn text(s: &str) -> Self {
        Self {
            rows: vec![s.to_string()],
            baseline: 0,
            grouped: false,
        }
    }

    fn height(&self) -> usize {
        self.rows.len()
    }

    fn width(&self) -> usize {
        self.rows
            .iter()
            .map(|r| display_width(r))
            .max()
            .unwrap_or(0)
    }

    /// Horizontal composition with baseline alignment. Zero-area parts
    /// (e.g. invisible `\left.` sides) never affect the geometry.
    fn hcat(parts: Vec<Canvas>) -> Canvas {
        let parts: Vec<Canvas> = parts
            .into_iter()
            .filter(|p| p.height() > 0 && p.width() > 0)
            .collect();
        if parts.is_empty() {
            return Canvas::empty();
        }
        let base = parts.iter().map(|p| p.baseline).max().unwrap_or(0);
        let below = parts
            .iter()
            .map(|p| p.height().saturating_sub(p.baseline))
            .max()
            .unwrap_or(0);
        let height = base + below;
        let width: usize = parts.iter().map(|p| p.width()).sum();
        let mut rows = vec![String::new(); height];
        for p in &parts {
            let w = p.width();
            for (i, row) in rows.iter_mut().enumerate() {
                let src = if i < base.saturating_sub(p.baseline) {
                    ""
                } else {
                    let li = i - (base - p.baseline);
                    if li < p.height() {
                        p.rows[li].as_str()
                    } else {
                        ""
                    }
                };
                row.push_str(&pad_to_width(src, w));
            }
        }
        Canvas {
            rows,
            baseline: base,
            grouped: false,
        }
        .normalize(width)
    }

    /// Vertical stack with an explicit baseline row.
    fn stack(parts: Vec<Canvas>, baseline: usize) -> Canvas {
        let width: usize = parts.iter().map(|p| p.width()).max().unwrap_or(0);
        let mut rows = Vec::new();
        for p in &parts {
            for r in &p.rows {
                rows.push(pad_to_width(r, width));
            }
        }
        let baseline = baseline.min(rows.len().saturating_sub(1));
        Canvas {
            rows,
            baseline,
            grouped: false,
        }
    }

    fn normalize(mut self, width: usize) -> Canvas {
        for r in &mut self.rows {
            *r = pad_to_width(r, width);
        }
        self
    }

    fn rule(width: usize) -> Canvas {
        Canvas::text(&"─".repeat(width.max(1)))
    }

    fn frac(num: Canvas, den: Canvas) -> Canvas {
        let w = num.width().max(den.width()).max(1);
        let rule = Canvas::rule(w);
        let base = num.height();
        Canvas::stack(vec![num, rule, den], base)
    }

    fn sup(base: Canvas, exp: Canvas) -> Canvas {
        let (bw, bh) = (base.width(), base.height());
        let (ew, eh) = (exp.width(), exp.height());
        let upper = Canvas::hcat(vec![Canvas::blank(bw, eh), exp]);
        let lower = Canvas::hcat(vec![base, Canvas::blank(ew, bh)]);
        Canvas::vstack(&upper, &lower)
    }

    fn vstack(upper: &Canvas, lower: &Canvas) -> Canvas {
        Canvas::stack(
            vec![upper.clone(), lower.clone()],
            upper.height() + lower.baseline,
        )
    }

    fn sub(base: Canvas, s: Canvas) -> Canvas {
        let (bw, bh) = (base.width(), base.height());
        let (ew, eh) = (s.width(), s.height());
        let upper = Canvas::hcat(vec![base, Canvas::blank(ew, bh)]);
        let lower = Canvas::hcat(vec![Canvas::blank(bw, eh), s]);
        Canvas::vstack(&upper, &lower)
    }
}

/// Try a single-line unicode script; None when any char is unmappable.
/// Already-scripted chars map to themselves, so nested scripts like
/// `e^{-x^2}` still collapse to one line.
fn map_script(s: &str, sup: bool) -> Option<String> {
    s.chars().map(|c| script_char(c, sup)).collect()
}

fn is_sup_char(c: char) -> bool {
    matches!(
        c,
        '⁰' | '¹'
            | '²'
            | '³'
            | '⁴'
            | '⁵'
            | '⁶'
            | '⁷'
            | '⁸'
            | '⁹'
            | 'ᵃ'
            | 'ᵇ'
            | 'ᶜ'
            | 'ᵈ'
            | 'ᵉ'
            | 'ᶠ'
            | 'ᵍ'
            | 'ʰ'
            | 'ⁱ'
            | 'ʲ'
            | 'ᵏ'
            | 'ˡ'
            | 'ᵐ'
            | 'ⁿ'
            | 'ᵒ'
            | 'ᵖ'
            | 'ʳ'
            | 'ˢ'
            | 'ᵗ'
            | 'ᵘ'
            | 'ᵛ'
            | 'ʷ'
            | 'ˣ'
            | 'ʸ'
            | 'ᶻ'
            | 'ᴬ'
            | 'ᴮ'
            | 'ᴰ'
            | 'ᴱ'
            | 'ᴳ'
            | 'ᴴ'
            | 'ᴵ'
            | 'ᴶ'
            | 'ᴷ'
            | 'ᴸ'
            | 'ᴹ'
            | 'ᴺ'
            | 'ᴼ'
            | 'ᴾ'
            | 'ᴿ'
            | 'ᵀ'
            | 'ᵁ'
            | 'ⱽ'
            | 'ᵂ'
            | '⁺'
            | '⁻'
            | '⁼'
            | '⁽'
            | '⁾'
    )
}

fn is_sub_char(c: char) -> bool {
    matches!(
        c,
        '₀' | '₁'
            | '₂'
            | '₃'
            | '₄'
            | '₅'
            | '₆'
            | '₇'
            | '₈'
            | '₉'
            | 'ₐ'
            | 'ₑ'
            | 'ₕ'
            | 'ᵢ'
            | 'ⱼ'
            | 'ₖ'
            | 'ₗ'
            | 'ₘ'
            | 'ₙ'
            | 'ₒ'
            | 'ₚ'
            | 'ᵣ'
            | 'ₛ'
            | 'ₜ'
            | 'ᵤ'
            | 'ᵥ'
            | 'ₓ'
            | '₊'
            | '₋'
            | '₌'
            | '₍'
            | '₎'
    )
}

fn script_char(c: char, sup: bool) -> Option<char> {
    if sup && is_sup_char(c) {
        return Some(c);
    }
    if !sup && is_sub_char(c) {
        return Some(c);
    }
    Some(match (c, sup) {
        ('0', true) => '⁰',
        ('1', true) => '¹',
        ('2', true) => '²',
        ('3', true) => '³',
        ('4', true) => '⁴',
        ('5', true) => '⁵',
        ('6', true) => '⁶',
        ('7', true) => '⁷',
        ('8', true) => '⁸',
        ('9', true) => '⁹',
        ('a', true) => 'ᵃ',
        ('b', true) => 'ᵇ',
        ('c', true) => 'ᶜ',
        ('d', true) => 'ᵈ',
        ('e', true) => 'ᵉ',
        ('f', true) => 'ᶠ',
        ('g', true) => 'ᵍ',
        ('h', true) => 'ʰ',
        ('i', true) => 'ⁱ',
        ('j', true) => 'ʲ',
        ('k', true) => 'ᵏ',
        ('l', true) => 'ˡ',
        ('m', true) => 'ᵐ',
        ('n', true) => 'ⁿ',
        ('o', true) => 'ᵒ',
        ('p', true) => 'ᵖ',
        ('r', true) => 'ʳ',
        ('s', true) => 'ˢ',
        ('t', true) => 'ᵗ',
        ('u', true) => 'ᵘ',
        ('v', true) => 'ᵛ',
        ('w', true) => 'ʷ',
        ('x', true) => 'ˣ',
        ('y', true) => 'ʸ',
        ('z', true) => 'ᶻ',
        ('A', true) => 'ᴬ',
        ('B', true) => 'ᴮ',
        ('D', true) => 'ᴰ',
        ('E', true) => 'ᴱ',
        ('G', true) => 'ᴳ',
        ('H', true) => 'ᴴ',
        ('I', true) => 'ᴵ',
        ('J', true) => 'ᴶ',
        ('K', true) => 'ᴷ',
        ('L', true) => 'ᴸ',
        ('M', true) => 'ᴹ',
        ('N', true) => 'ᴺ',
        ('O', true) => 'ᴼ',
        ('P', true) => 'ᴾ',
        ('R', true) => 'ᴿ',
        ('T', true) => 'ᵀ',
        ('U', true) => 'ᵁ',
        ('V', true) => 'ⱽ',
        ('W', true) => 'ᵂ',
        ('+', true) => '⁺',
        ('-', true) => '⁻',
        ('=', true) => '⁼',
        ('(', true) => '⁽',
        (')', true) => '⁾',
        ('0', false) => '₀',
        ('1', false) => '₁',
        ('2', false) => '₂',
        ('3', false) => '₃',
        ('4', false) => '₄',
        ('5', false) => '₅',
        ('6', false) => '₆',
        ('7', false) => '₇',
        ('8', false) => '₈',
        ('9', false) => '₉',
        ('a', false) => 'ₐ',
        ('e', false) => 'ₑ',
        ('h', false) => 'ₕ',
        ('i', false) => 'ᵢ',
        ('j', false) => 'ⱼ',
        ('k', false) => 'ₖ',
        ('l', false) => 'ₗ',
        ('m', false) => 'ₘ',
        ('n', false) => 'ₙ',
        ('o', false) => 'ₒ',
        ('p', false) => 'ₚ',
        ('r', false) => 'ᵣ',
        ('s', false) => 'ₛ',
        ('t', false) => 'ₜ',
        ('u', false) => 'ᵤ',
        ('v', false) => 'ᵥ',
        ('x', false) => 'ₓ',
        ('+', false) => '₊',
        ('-', false) => '₋',
        ('=', false) => '₌',
        ('(', false) => '₍',
        (')', false) => '₎',
        _ => return None,
    })
}

/// Environments whose rows the grid engine cannot lay out: unwrap them so
/// each `\\` row renders on its own. Matrix environments are left intact
/// (their `\\` separators belong to the grid).
fn display_rows(content: &str) -> Vec<String> {
    let mut s = content.to_string();
    for env in [
        "aligned",
        "gathered",
        "split",
        "align",
        "align*",
        "equation",
        "equation*",
        "gather",
        "gather*",
        "multline",
        "multline*",
    ] {
        s = s.replace(&format!("\\begin{{{env}}}"), "");
        s = s.replace(&format!("\\end{{{env}}}"), "");
    }
    const VERBATIM: [&str; 7] = [
        "pmatrix", "bmatrix", "vmatrix", "Vmatrix", "matrix", "array", "cases",
    ];
    if VERBATIM
        .iter()
        .any(|e| s.contains(&format!("\\begin{{{e}}}")))
    {
        return vec![s];
    }
    let mut rows = Vec::new();
    for line in s.split('\n') {
        for sub in line.split("\\\\") {
            rows.push(sub.replace('&', " "));
        }
    }
    rows
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Delim {
    Paren,
    Bracket,
    Brace,
    Bar,
    DoubleBar,
    Angle,
    Ceil,
    Floor,
    Dot,
}

impl Delim {
    /// Vertical pieces (top, middle, bottom) for a grown delimiter side.
    /// Ceil hooks sit at the top on both sides, floor hooks at the bottom.
    fn pieces(self, right: bool) -> (&'static str, &'static str, &'static str) {
        match (self, right) {
            (Delim::Paren, false) => ("⎛", "⎜", "⎝"),
            (Delim::Paren, true) => ("⎞", "⎟", "⎠"),
            (Delim::Bracket, false) => ("⎡", "⎢", "⎣"),
            (Delim::Bracket, true) => ("⎤", "⎥", "⎦"),
            (Delim::Brace, false) => ("⎧", "⎪", "⎩"),
            (Delim::Brace, true) => ("⎫", "⎪", "⎭"),
            (Delim::Bar, _) => ("│", "│", "│"),
            (Delim::DoubleBar, _) => ("‖", "‖", "‖"),
            (Delim::Angle, false) => ("⟨", "⟨", "⟨"),
            (Delim::Angle, true) => ("⟩", "⟩", "⟩"),
            (Delim::Ceil, false) => ("⌈", "⎮", "⎮"),
            (Delim::Ceil, true) => ("⌉", "⎮", "⎮"),
            (Delim::Floor, false) => ("⎮", "⎮", "⎣"),
            (Delim::Floor, true) => ("⎮", "⎮", "⎦"),
            (Delim::Dot, _) => ("", "", ""),
        }
    }

    fn single(self, right: bool) -> &'static str {
        match (self, right) {
            (Delim::Paren, false) => "(",
            (Delim::Paren, true) => ")",
            (Delim::Bracket, false) => "[",
            (Delim::Bracket, true) => "]",
            (Delim::Brace, false) => "{",
            (Delim::Brace, true) => "}",
            (Delim::Bar, _) => "|",
            (Delim::DoubleBar, _) => "‖",
            (Delim::Angle, false) => "⟨",
            (Delim::Angle, true) => "⟩",
            (Delim::Ceil, false) => "⌈",
            (Delim::Ceil, true) => "⌉",
            (Delim::Floor, false) => "⌊",
            (Delim::Floor, true) => "⎦",
            (Delim::Dot, _) => "",
        }
    }

    fn grow(self, height: usize, baseline: usize, right: bool) -> Canvas {
        if self == Delim::Dot || height == 0 {
            return Canvas::blank(0, height);
        }
        if height == 1 {
            return Canvas {
                rows: vec![self.single(right).to_string()],
                baseline: 0,
                grouped: false,
            };
        }
        let (top, mid, bot) = self.pieces(right);
        let mut rows = Vec::new();
        for i in 0..height {
            rows.push(
                if i == 0 {
                    top
                } else if i + 1 == height {
                    bot
                } else {
                    mid
                }
                .to_string(),
            );
        }
        Canvas {
            rows,
            baseline,
            grouped: false,
        }
    }
}

impl Canvas {
    fn sqrt(body: Canvas, index: Option<Canvas>) -> Canvas {
        let w = body.width().max(1);
        let idx_text = index
            .map(|c| {
                c.rows
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|n| !n.is_empty());
        let idx_w = idx_text
            .as_ref()
            .map(|n| display_width(n))
            .unwrap_or(1)
            .max(1);
        let mut top = String::new();
        match &idx_text {
            Some(n) => {
                top.push_str(n);
                top.push_str(&" ".repeat(idx_w.saturating_sub(display_width(n))));
            }
            None => {
                top.push(' ');
                top.push_str(&" ".repeat(idx_w.saturating_sub(1)));
            }
        }
        top.push_str(&"─".repeat(w));
        let mut rows = vec![top];
        let n = body.height();
        for (i, r) in body.rows.iter().enumerate() {
            let stem = if n == 1 {
                "√".to_string()
            } else if i + 1 == n {
                format!("√{}", " ".repeat(idx_w.saturating_sub(1)))
            } else {
                format!("│{}", " ".repeat(idx_w.saturating_sub(1)))
            };
            rows.push(format!("{stem}{r}"));
        }
        let width = idx_w + w;
        for r in rows.iter_mut() {
            *r = pad_to_width(r, width);
        }
        Canvas {
            rows,
            baseline: 1 + body.baseline,
            grouped: false,
        }
    }

    fn overline(body: Canvas) -> Canvas {
        let rule = Canvas::rule(body.width().max(1));
        Canvas::stack(vec![rule, body.clone()], 1 + body.baseline)
    }

    fn underline(body: Canvas) -> Canvas {
        let rule = Canvas::rule(body.width().max(1));
        let base = body.baseline;
        Canvas::stack(vec![body, rule], base)
    }

    fn accent(body: Canvas, mark: char) -> Canvas {
        let w = body.width().max(1);
        let mw = display_width(&mark.to_string()).max(1);
        let mut top = " ".repeat(w);
        place_str(&mut top, &mark.to_string(), w.saturating_sub(mw) / 2);
        Canvas::stack(vec![Canvas::text(&top), body.clone()], 1 + body.baseline)
    }

    /// Grid with grown side delimiters; cells baseline-align per row and
    /// center within their column.
    fn matrix(cells: Vec<Vec<Canvas>>, open: Delim, close: Delim) -> Canvas {
        if cells.is_empty() {
            return Canvas::text("");
        }
        let cols = cells.iter().map(|r| r.len()).max().unwrap_or(0).max(1);
        let mut maxw = vec![0usize; cols];
        for row in &cells {
            for (j, cell) in row.iter().enumerate() {
                maxw[j] = maxw[j].max(cell.width());
            }
        }
        let centered: Vec<Vec<Canvas>> = cells
            .iter()
            .map(|row| {
                (0..cols)
                    .map(|j| {
                        let cell = row.get(j).cloned().unwrap_or_else(Canvas::empty);
                        let w = cell.width();
                        let target = maxw[j];
                        if w >= target {
                            return cell;
                        }
                        let left = (target - w) / 2;
                        let right = target - w - left;
                        let rows = cell
                            .rows
                            .iter()
                            .map(|r| format!("{}{}{}", " ".repeat(left), r, " ".repeat(right)))
                            .collect();
                        Canvas {
                            rows,
                            baseline: cell.baseline,
                            grouped: false,
                        }
                    })
                    .collect()
            })
            .collect();
        let mut row_blocks: Vec<Canvas> = Vec::new();
        for row in &centered {
            let mut parts: Vec<Canvas> = Vec::new();
            for j in 0..cols {
                if j > 0 {
                    parts.push(Canvas::blank(2, 1));
                }
                parts.push(row.get(j).cloned().unwrap_or_else(Canvas::empty));
            }
            row_blocks.push(Canvas::hcat(parts));
        }
        let total_h: usize = row_blocks.iter().map(|r| r.height()).sum();
        let mid = total_h / 2;
        let grid = Canvas::stack(row_blocks, mid);
        let h = grid.height().max(1);
        let left = open.grow(h, mid.min(h.saturating_sub(1)), false);
        let right = close.grow(h, mid.min(h.saturating_sub(1)), true);
        Canvas::hcat(vec![left, grid, right])
    }
}

fn place_str(line: &mut String, s: &str, col: usize) {
    let mut chars: Vec<char> = line.chars().collect();
    while chars.len() < col + s.chars().count() {
        chars.push(' ');
    }
    for (k, c) in s.chars().enumerate() {
        chars[col + k] = c;
    }
    *line = chars.into_iter().collect();
}

fn bold_char(c: char) -> Option<char> {
    let cp = c as u32;
    let mapped = if ('A'..='Z').contains(&c) {
        0x1D400 + (cp - 'A' as u32)
    } else if ('a'..='z').contains(&c) {
        0x1D41A + (cp - 'a' as u32)
    } else if ('0'..='9').contains(&c) {
        0x1D7CE + (cp - '0' as u32)
    } else {
        return None;
    };
    char::from_u32(mapped)
}

fn script_capital(c: char) -> Option<char> {
    Some(match c {
        'A' => '𝒜',
        'B' => 'ℬ',
        'C' => '𝒞',
        'D' => '𝒟',
        'E' => 'ℰ',
        'F' => 'ℱ',
        'G' => '𝒢',
        'H' => 'ℋ',
        'I' => 'ℐ',
        'J' => '𝒥',
        'K' => '𝒦',
        'L' => 'ℒ',
        'M' => 'ℳ',
        'N' => '𝒩',
        'O' => '𝒪',
        'P' => '𝒫',
        'Q' => '𝒬',
        'R' => 'ℛ',
        'S' => '𝒮',
        'T' => '𝒯',
        'U' => '𝒰',
        'V' => '𝒱',
        'W' => '𝒲',
        'X' => '𝒳',
        'Y' => '𝒴',
        'Z' => '𝒵',
        _ => return None,
    })
}

fn is_display_op(base: &Canvas) -> bool {
    if base.height() != 1 || base.grouped {
        return false;
    }
    matches!(
        base.rows.first().map(String::as_str).unwrap_or(""),
        "∑" | "∏"
            | "∐"
            | "∫"
            | "∮"
            | "∬"
            | "∭"
            | "⋃"
            | "⋂"
            | "⋁"
            | "⋀"
            | "⊕"
            | "⊗"
            | "⊎"
            | "lim"
            | "limsup"
            | "liminf"
            | "max"
            | "min"
            | "sup"
            | "inf"
            | "det"
            | "gcd"
            | "Pr"
    )
}

/// Centered stack for a display operator with above/below limits.
fn center_stack(base: Canvas, above: Option<Canvas>, below: Option<Canvas>) -> Canvas {
    let mut w = base.width().max(1);
    for opt in [&above, &below] {
        if let Some(c) = opt {
            w = w.max(c.width());
        }
    }
    let center = |c: &Canvas| {
        let mut rows = Vec::new();
        for r in &c.rows {
            let cw = display_width(r);
            let left = w.saturating_sub(cw) / 2;
            rows.push(format!(
                "{}{}{}",
                " ".repeat(left),
                r,
                " ".repeat(w.saturating_sub(cw + left))
            ));
        }
        Canvas {
            rows,
            baseline: c.baseline,
            grouped: false,
        }
    };
    let mut parts = Vec::new();
    let mut baseline = 0;
    if let Some(a) = &above {
        baseline += a.height();
        parts.push(center(a));
    }
    parts.push(center(&base));
    if let Some(b) = &below {
        parts.push(center(b));
    }
    Canvas::stack(parts, baseline)
}

fn apply_script(base: Canvas, script: Canvas, sup: bool, display: bool) -> Canvas {
    if display && is_display_op(&base) {
        return if sup {
            center_stack(base, Some(script), None)
        } else {
            center_stack(base, None, Some(script))
        };
    }
    if base.height() == 1 && script.height() == 1 {
        let b = base.rows.first().cloned().unwrap_or_default();
        let s = script.rows.first().cloned().unwrap_or_default();
        if !s.trim().is_empty() {
            if let Some(m) = map_script(s.trim(), sup) {
                return Canvas::text(&format!("{b}{m}"));
            }
        }
    }
    if sup {
        Canvas::sup(base, script)
    } else {
        Canvas::sub(base, script)
    }
}

fn apply_prime(base: Canvas, count: usize) -> Canvas {
    if base.height() == 1 {
        let mark = match count {
            1 => "′",
            2 => "″",
            3 => "‴",
            _ => "⁗",
        };
        let mut row = base.rows.into_iter().next().unwrap_or_default();
        row.push_str(mark);
        return Canvas::text(&row);
    }
    base
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Term {
    Eof,
    Brace,
    Amp,
    DSlash,
    EndEnv,
    Right(Delim),
}

struct MathParser {
    chars: Vec<char>,
    pos: usize,
    display: bool,
}

impl MathParser {
    fn new(s: &str, display: bool) -> Self {
        Self {
            chars: s.chars().collect(),
            pos: 0,
            display,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    /// Command name at `\` without consuming (`\foo` or single char).
    fn peek_cmd_name(&self) -> String {
        let mut j = self.pos + 1;
        if j < self.chars.len() && self.chars[j].is_ascii_alphabetic() {
            let start = j;
            while j < self.chars.len() && self.chars[j].is_ascii_alphabetic() {
                j += 1;
            }
            return self.chars[start..j].iter().collect();
        }
        self.chars.get(j).map(|c| c.to_string()).unwrap_or_default()
    }

    /// Parse units until a terminator (consumed and reported).
    fn parse_until(&mut self, split_cells: bool, in_left: bool) -> (Canvas, Term) {
        let mut parts = Vec::new();
        loop {
            self.skip_spaces();
            match self.peek() {
                None => return (join_atoms(parts), Term::Eof),
                Some('}') => {
                    self.pos += 1;
                    return (join_atoms(parts), Term::Brace);
                }
                Some('&') => {
                    if split_cells {
                        self.pos += 1;
                        return (join_atoms(parts), Term::Amp);
                    }
                    parts.push(Canvas::text("&"));
                    self.pos += 1;
                }
                Some('\\') if self.peek2() == Some('\\') => {
                    if split_cells {
                        self.pos += 2;
                        return (join_atoms(parts), Term::DSlash);
                    }
                    parts.push(Canvas::text(" "));
                    self.pos += 2;
                }
                Some('\\') => {
                    let name = self.peek_cmd_name();
                    if in_left && name == "right" {
                        self.pos += 1 + name.len();
                        let d = self.parse_delim();
                        return (join_atoms(parts), Term::Right(d));
                    }
                    if name == "end" {
                        self.pos += 1 + name.len();
                        self.skip_spaces();
                        if self.peek() == Some('{') {
                            self.pos += 1;
                            while self.peek().is_some_and(|c| c != '}') {
                                self.pos += 1;
                            }
                            if self.peek() == Some('}') {
                                self.pos += 1;
                            }
                        }
                        return (join_atoms(parts), Term::EndEnv);
                    }
                    parts.push(self.parse_unit());
                }
                _ => parts.push(self.parse_unit()),
            }
        }
    }

    /// One atom with postfix scripts and primes.
    fn parse_unit(&mut self) -> Canvas {
        let mut base = self.parse_atom();
        // Gather scripts first: a display operator carrying both gets one
        // centered triple instead of two lopsided stacks.
        let mut sub: Option<Canvas> = None;
        let mut sup: Option<Canvas> = None;
        let mut primes = 0;
        loop {
            self.skip_spaces();
            match self.peek() {
                Some('^') => {
                    self.pos += 1;
                    match self.parse_script_unit() {
                        Some(s) => {
                            if sup.is_some() {
                                base = Canvas::hcat(vec![base, Canvas::text("^"), s]);
                            } else {
                                sup = Some(s);
                            }
                        }
                        None => {
                            base = Canvas::hcat(vec![base, Canvas::text("^")]);
                            break;
                        }
                    }
                }
                Some('_') => {
                    self.pos += 1;
                    match self.parse_script_unit() {
                        Some(s) => {
                            if sub.is_some() {
                                base = Canvas::hcat(vec![base, Canvas::text("_"), s]);
                            } else {
                                sub = Some(s);
                            }
                        }
                        None => {
                            base = Canvas::hcat(vec![base, Canvas::text("_")]);
                            break;
                        }
                    }
                }
                Some('\'') => {
                    while self.peek() == Some('\'') {
                        self.pos += 1;
                        primes += 1;
                    }
                }
                _ => break,
            }
        }
        if primes > 0 {
            base = apply_prime(base, primes);
        }
        if self.display && (sub.is_some() || sup.is_some()) && is_display_op(&base) {
            return center_stack(base, sup, sub);
        }
        let mut out = base;
        if let Some(s) = sub {
            out = apply_script(out, s, false, self.display);
        }
        if let Some(s) = sup {
            out = apply_script(out, s, true, self.display);
        }
        out
    }

    /// `{...}`, `\command`, or a single char; None when absent.
    fn parse_script_unit(&mut self) -> Option<Canvas> {
        self.skip_spaces();
        match self.peek() {
            None => None,
            Some('{') => {
                self.pos += 1;
                let (c, _) = self.parse_until(false, false);
                Some(c)
            }
            Some('\\') => Some(self.parse_atom()),
            Some('^') | Some('_') | Some('&') | Some('}') | Some('$') => None,
            Some(c) => {
                self.pos += c.len_utf8();
                Some(Canvas::text(&c.to_string()))
            }
        }
    }

    fn parse_group(&mut self) -> Canvas {
        // Current char is '{'.
        self.pos += 1;
        let (c, _) = self.parse_until(false, false);
        let mut c = c;
        c.grouped = true;
        c
    }

    fn parse_atom(&mut self) -> Canvas {
        match self.peek() {
            None => Canvas::empty(),
            Some('{') => self.parse_group(),
            Some('\\') => self.parse_command(),
            Some(c) if c.is_ascii_digit() => {
                let start = self.pos;
                while self.peek().is_some_and(|d| d.is_ascii_digit()) {
                    self.pos += 1;
                }
                Canvas::text(&self.chars[start..self.pos].iter().collect::<String>())
            }
            Some(c) => {
                self.pos += c.len_utf8();
                Canvas::text(&c.to_string())
            }
        }
    }

    fn read_cmd_name(&mut self) -> String {
        // Current char is '\\'.
        self.pos += 1;
        if self.pos < self.chars.len() && self.chars[self.pos].is_ascii_alphabetic() {
            let start = self.pos;
            while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_alphabetic() {
                self.pos += 1;
            }
            return self.chars[start..self.pos].iter().collect();
        }
        match self.chars.get(self.pos) {
            Some(c) => {
                let s = c.to_string();
                self.pos += 1;
                s
            }
            None => String::new(),
        }
    }

    fn parse_delim(&mut self) -> Delim {
        self.skip_spaces();
        if self.peek() == Some('\\') {
            let save = self.pos;
            self.pos += 1;
            let start = self.pos;
            while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_alphabetic() {
                self.pos += 1;
            }
            if start < self.pos {
                let name: String = self.chars[start..self.pos].iter().collect();
                return match name.as_str() {
                    "langle" => Delim::Angle,
                    "rangle" => Delim::Angle,
                    "lceil" => Delim::Ceil,
                    "rceil" => Delim::Ceil,
                    "lfloor" => Delim::Floor,
                    "rfloor" => Delim::Floor,
                    "vert" => Delim::Bar,
                    "Vert" => Delim::DoubleBar,
                    _ => Delim::Dot,
                };
            }
            match self.chars.get(self.pos) {
                Some('|') => {
                    self.pos += 1;
                    return Delim::DoubleBar;
                }
                Some('{') => {
                    self.pos += 1;
                    return Delim::Brace;
                }
                Some('}') => {
                    self.pos += 1;
                    return Delim::Brace;
                }
                _ => {
                    self.pos = save + 1;
                }
            }
        }
        match self.peek() {
            Some('(') => {
                self.pos += 1;
                Delim::Paren
            }
            Some(')') => {
                self.pos += 1;
                Delim::Paren
            }
            Some('[') => {
                self.pos += 1;
                Delim::Bracket
            }
            Some(']') => {
                self.pos += 1;
                Delim::Bracket
            }
            Some('|') => {
                self.pos += 1;
                Delim::Bar
            }
            Some('.') => {
                self.pos += 1;
                Delim::Dot
            }
            Some('<') => {
                self.pos += 1;
                Delim::Angle
            }
            Some('>') => {
                self.pos += 1;
                Delim::Angle
            }
            _ => Delim::Dot,
        }
    }

    fn parse_left(&mut self, open: Delim) -> Canvas {
        let mut parts = Vec::new();
        let close = loop {
            let (c, term) = self.parse_until(false, true);
            if c.height() > 0 {
                parts.push(c);
            }
            match term {
                Term::Right(d) => break d,
                Term::Amp | Term::DSlash => {
                    parts.push(Canvas::text(" "));
                }
                Term::Eof | Term::Brace | Term::EndEnv => break Delim::Dot,
            }
        };
        let inner = join_atoms(parts);
        let h = inner.height().max(1);
        let mid = inner.baseline.min(h.saturating_sub(1));
        Canvas::hcat(vec![
            open.grow(h, mid, false),
            inner,
            close.grow(h, mid, true),
        ])
    }

    fn parse_matrix(&mut self, open: Delim, close: Delim) -> Canvas {
        let mut rows: Vec<Vec<Canvas>> = Vec::new();
        let mut current: Vec<Canvas> = Vec::new();
        loop {
            let (c, term) = self.parse_until(true, false);
            current.push(c);
            match term {
                Term::Amp => {}
                Term::DSlash | Term::EndEnv | Term::Eof | Term::Brace => {
                    rows.push(std::mem::take(&mut current));
                    if term != Term::DSlash {
                        break;
                    }
                }
                Term::Right(_) => {
                    rows.push(std::mem::take(&mut current));
                    break;
                }
            }
        }
        rows.retain(|r| !(r.len() == 1 && r[0].height() == 0));
        Canvas::matrix(rows, open, close)
    }

    fn parse_command(&mut self) -> Canvas {
        let name = self.read_cmd_name();
        if name.len() == 1
            && !"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ".contains(&name[..])
        {
            return match name.as_str() {
                " " | "," | ";" | ":" => Canvas::text(" "),
                "!" => Canvas::empty(),
                "\\" => Canvas::text(" "),
                _ => Canvas::text(&name),
            };
        }
        match name.as_str() {
            "left" => {
                let d = self.parse_delim();
                return self.parse_left(d);
            }
            "right" => return Canvas::empty(),
            "frac" => {
                let a = self.braced_layout();
                let b = self.braced_layout();
                match (a, b) {
                    (Some(a), Some(b)) => return Canvas::frac(a, b),
                    _ => return Canvas::text("frac"),
                }
            }
            "sqrt" => {
                self.skip_spaces();
                let mut index = None;
                if self.peek() == Some('[') {
                    self.pos += 1;
                    let mut depth = 0;
                    let mut buf = String::new();
                    loop {
                        match self.peek() {
                            None => break,
                            Some(']') if depth == 0 => {
                                self.pos += 1;
                                break;
                            }
                            Some('[') => {
                                depth += 1;
                                buf.push('[');
                                self.pos += 1;
                            }
                            Some(']') => {
                                depth -= 1;
                                buf.push(']');
                                self.pos += 1;
                            }
                            Some(c) => {
                                buf.push(c);
                                self.pos += c.len_utf8();
                            }
                        }
                    }
                    index = Some(layout_math(&buf, self.display));
                }
                return match self.braced_layout() {
                    Some(body) => Canvas::sqrt(body, index),
                    None => Canvas::text("√"),
                };
            }
            "sum" | "prod" | "coprod" | "int" | "oint" | "iint" | "iiint" | "bigcup" | "bigcap"
            | "bigvee" | "bigwedge" | "bigoplus" | "bigotimes" | "biguplus" => {
                return Canvas::text(big_op_glyph(&name));
            }
            "lim" | "limsup" | "liminf" | "max" | "min" | "sup" | "inf" | "det" | "gcd" | "Pr" => {
                return Canvas::text(&name);
            }
            "sin" | "cos" | "tan" | "sec" | "csc" | "cot" | "sinh" | "cosh" | "tanh" | "arcsin"
            | "arccos" | "arctan" | "ln" | "lg" | "log" | "exp" | "arg" | "deg" | "dim" | "hom"
            | "ker" => {
                return Canvas::text(&name);
            }
            "begin" => {
                self.skip_spaces();
                let env = match take_braced_arg(&self.chars, self.pos) {
                    Some((e, j)) => {
                        self.pos = j;
                        e
                    }
                    None => return Canvas::text("begin"),
                };
                let (open, close) = match env.as_str() {
                    "pmatrix" => (Delim::Paren, Delim::Paren),
                    "bmatrix" => (Delim::Bracket, Delim::Bracket),
                    "vmatrix" => (Delim::Bar, Delim::Bar),
                    "Vmatrix" => (Delim::DoubleBar, Delim::DoubleBar),
                    "matrix" => (Delim::Dot, Delim::Dot),
                    "array" => (Delim::Dot, Delim::Dot),
                    "cases" => (Delim::Brace, Delim::Dot),
                    _ => {
                        // Unknown environment: flow content through.
                        let (c, _) = self.parse_until(false, false);
                        return c;
                    }
                };
                return self.parse_matrix(open, close);
            }
            "binom" | "dbinom" => {
                let a = self.braced_layout();
                let b = self.braced_layout();
                match (a, b) {
                    (Some(a), Some(b)) => {
                        return Canvas::matrix(vec![vec![a], vec![b]], Delim::Paren, Delim::Paren)
                    }
                    _ => return Canvas::text("binom"),
                }
            }
            "end" => return Canvas::empty(),
            "text" => {
                return match take_braced_arg(&self.chars, self.pos) {
                    Some((t, j)) => {
                        self.pos = j;
                        Canvas::text(&t)
                    }
                    None => Canvas::empty(),
                };
            }
            "mathrm" | "mathit" | "boldsymbol" | "pmb" => {
                return self.braced_layout().unwrap_or_else(Canvas::empty);
            }
            "mathbf" => {
                return match self.braced_layout() {
                    Some(c) if c.height() == 1 => Canvas::text(
                        &c.rows
                            .into_iter()
                            .next()
                            .unwrap_or_default()
                            .chars()
                            .map(|ch| bold_char(ch).unwrap_or(ch))
                            .collect::<String>(),
                    ),
                    Some(c) => c,
                    None => Canvas::empty(),
                };
            }
            "mathbb" => {
                return match self.braced_layout() {
                    Some(c) if c.height() == 1 => Canvas::text(
                        &c.rows
                            .into_iter()
                            .next()
                            .unwrap_or_default()
                            .chars()
                            .map(|ch| blackboard(ch).unwrap_or(ch))
                            .collect::<String>(),
                    ),
                    Some(c) => c,
                    None => Canvas::empty(),
                };
            }
            "mathcal" => {
                return match self.braced_layout() {
                    Some(c) if c.height() == 1 => Canvas::text(
                        &c.rows
                            .into_iter()
                            .next()
                            .unwrap_or_default()
                            .chars()
                            .map(|ch| {
                                script_capital(ch)
                                    .or_else(|| {
                                        if ('a'..='z').contains(&ch) {
                                            char::from_u32(0x1D4B6 + (ch as u32 - 'a' as u32))
                                        } else {
                                            None
                                        }
                                    })
                                    .unwrap_or(ch)
                            })
                            .collect::<String>(),
                    ),
                    Some(c) => c,
                    None => Canvas::empty(),
                };
            }
            "mathfrak" => {
                return self.braced_layout().unwrap_or_else(Canvas::empty);
            }
            "overline" => {
                return match self.braced_layout() {
                    Some(b) => Canvas::overline(b),
                    None => Canvas::empty(),
                };
            }
            "underline" => {
                return match self.braced_layout() {
                    Some(b) => Canvas::underline(b),
                    None => Canvas::empty(),
                };
            }
            "hat" => return Canvas::accent(self.script_atom(), '^'),
            "bar" => return Canvas::accent(self.script_atom(), '‾'),
            "vec" => return Canvas::accent(self.script_atom(), '→'),
            "dot" => return Canvas::accent(self.script_atom(), '˙'),
            "ddot" => return Canvas::accent(self.script_atom(), '¨'),
            "tilde" => return Canvas::accent(self.script_atom(), '~'),
            "check" => return Canvas::accent(self.script_atom(), '✓'),
            "not" => {
                let mut a = self.parse_atom();
                if a.height() == 1 {
                    if let Some(row) = a.rows.first_mut() {
                        row.push('̸');
                    }
                }
                return a;
            }
            "color" => {
                if let Some((_, j)) = take_braced_arg(&self.chars, self.pos) {
                    self.pos = j;
                }
                return self.parse_atom();
            }
            "middle" => {
                self.skip_spaces();
                if self.peek() == Some('\\') {
                    let save = self.pos;
                    self.pos += 1;
                    if self.peek() == Some('|') {
                        self.pos += 1;
                        return Canvas::text("‖");
                    }
                    self.pos = save;
                }
                if self.peek() == Some('|') {
                    self.pos += 1;
                    return Canvas::text("|");
                }
                return Canvas::empty();
            }
            "label" | "tag" | "nonumber" | "displaystyle" | "textstyle" | "scriptstyle"
            | "limits" | "nolimits" => {
                self.skip_spaces();
                if self.peek() == Some('[') {
                    self.pos += 1;
                    while self.peek().is_some_and(|c| c != ']') {
                        self.pos += 1;
                    }
                    if self.peek() == Some(']') {
                        self.pos += 1;
                    }
                }
                self.skip_spaces();
                if self.peek() == Some('{') {
                    if let Some((_, j)) = take_braced_arg(&self.chars, self.pos) {
                        self.pos = j;
                    }
                } else if name == "tag" {
                    while self.peek().is_some_and(|c| c != ' ' && c != '$') {
                        self.pos += 1;
                    }
                }
                return Canvas::empty();
            }
            "hspace" | "vspace" | "quad" | "qquad" => {
                if let Some((_, j)) = take_braced_arg(&self.chars, self.pos) {
                    self.pos = j;
                }
                let n = if name == "qquad" { 4 } else { 1 };
                return Canvas::text(&" ".repeat(n));
            }
            _ => {}
        }
        if let Some(sym) = latex_symbol(&name) {
            return Canvas::text(sym);
        }
        // Unknown command: keep the argument like the legacy translator.
        if self.peek() == Some('{') {
            if let Some((arg, j)) = take_braced_arg(&self.chars, self.pos) {
                self.pos = j;
                return layout_math(&arg, self.display);
            }
        }
        Canvas::text(&name)
    }

    /// Next atom for accents (single token, group, or command).
    fn script_atom(&mut self) -> Canvas {
        self.skip_spaces();
        match self.peek() {
            None => Canvas::empty(),
            Some('{') => self.parse_group(),
            _ => self.parse_atom(),
        }
    }

    /// `{...}` parsed to a canvas; None when absent.
    fn braced_layout(&mut self) -> Option<Canvas> {
        self.skip_spaces();
        if self.peek() != Some('{') {
            return None;
        }
        self.pos += 1;
        let (c, _) = self.parse_until(false, false);
        Some(c)
    }
}

fn big_op_glyph(name: &str) -> &'static str {
    match name {
        "sum" => "∑",
        "prod" => "∏",
        "coprod" => "∐",
        "int" => "∫",
        "oint" => "∮",
        "iint" => "∬",
        "iiint" => "∭",
        "bigcup" => "⋃",
        "bigcap" => "⋂",
        "bigvee" => "⋁",
        "bigwedge" => "⋀",
        "bigoplus" => "⊕",
        "bigotimes" => "⊗",
        "biguplus" => "⊎",
        _ => "?",
    }
}

/// Parse one math row into a 2D canvas. Never fails: unknown input
/// degrades to literal text. In display mode big operators stack their
/// limits above/below; inline keeps them compact.
pub(crate) fn layout_math(s: &str, display: bool) -> Canvas {
    let mut parts = Vec::new();
    let mut p = MathParser::new(s, display);
    loop {
        let (c, term) = p.parse_until(false, false);
        if c.height() > 0 {
            parts.push(c);
        }
        if term == Term::Eof {
            break;
        }
    }
    join_atoms(parts)
}
pub(crate) fn render_display_math(content: &str, max_width: usize) -> Option<Vec<String>> {
    use unicode_width::UnicodeWidthStr as _;
    let content = content
        .replace("\\tfrac", "\\frac")
        .replace("\\dfrac", "\\frac");
    let mut rows: Vec<String> = Vec::new();
    for row in display_rows(&content) {
        if row.trim().is_empty() {
            continue;
        }
        let canvas = layout_math(&row, true);
        if canvas.height() == 0 {
            return None;
        }
        for line in &canvas.rows {
            let line = line.trim_end().to_string();
            if line.width() > max_width {
                return None;
            }
            rows.push(line);
        }
    }
    if rows.is_empty() {
        return None;
    }
    Some(rows)
}

pub(crate) fn translate_bare_latex_chunk(s: &str) -> String {
    pub(crate) fn braced_arg(chars: &[char], i: &mut usize) -> Option<String> {
        while *i < chars.len() && chars[*i].is_whitespace() {
            *i += 1;
        }
        if chars.get(*i) == Some(&'{') {
            *i += 1;
            let mut depth = 1;
            let start = *i;
            while *i < chars.len() {
                match chars[*i] {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            let inner: String = chars[start..*i].iter().collect();
                            *i += 1;
                            return Some(translate_bare_latex_chunk(&inner));
                        }
                    }
                    _ => {}
                }
                *i += 1;
            }
            return None;
        }

        chars.get(*i).map(|c| {
            *i += 1;
            c.to_string()
        })
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\\' {
            out.push(chars[i]);
            i += 1;
            continue;
        }

        let mut j = i + 1;
        while j < chars.len() && chars[j].is_ascii_alphabetic() {
            j += 1;
        }
        if j == i + 1 {
            match chars.get(j) {
                None => {
                    out.push('\\');
                    i += 1;
                }

                Some('\\') => {
                    out.push_str("\\\\");
                    i = j + 1;
                }
                Some(',') | Some(';') | Some(':') | Some(' ') => {
                    out.push(' ');
                    i = j + 1;
                }

                Some('!') => {
                    i = j + 1;
                }

                Some(c) => {
                    out.push(*c);
                    i = j + 1;
                }
            }
            continue;
        }
        let name: String = chars[i + 1..j].iter().collect();
        if name == "left" || name == "right" {
            i = j;
            continue;
        }
        if name == "frac" {
            let mut k = j;
            let a = braced_arg(&chars, &mut k);
            let b = braced_arg(&chars, &mut k);
            if let (Some(a), Some(b)) = (a, b) {
                out.push_str(&format!("({a})/({b})"));
                i = k;
            } else {
                out.push_str("\\frac");
                i = j;
            }
            continue;
        }
        if name == "sqrt" {
            let mut k = j;
            if let Some(x) = braced_arg(&chars, &mut k) {
                out.push_str(&format!("√({x})"));
                i = k;
            } else {
                out.push_str("\\sqrt");
                i = j;
            }
            continue;
        }
        if name == "text" || name == "mathrm" || name == "mathbf" || name == "mathit" {
            let mut k = j;
            if let Some(x) = braced_arg(&chars, &mut k) {
                out.push_str(&x);
                i = k;
            } else {
                out.push('\\');
                out.push_str(&name);
                i = j;
            }
            continue;
        }
        if name == "mathbb" {
            let mut k = j;
            if let Some(x) = braced_arg(&chars, &mut k) {
                let mapped: String = x.chars().map(|ch| blackboard(ch).unwrap_or(ch)).collect();
                out.push_str(&mapped);
                i = k;
            } else {
                out.push_str("\\mathbb");
                i = j;
            }
            continue;
        }
        if name == "hspace" {
            let mut k = j;
            if braced_arg(&chars, &mut k).is_some() {
                out.push(' ');
                i = k;
            } else {
                out.push_str("\\hspace");
                i = j;
            }
            continue;
        }
        if name == "vspace" {
            let mut k = j;
            if braced_arg(&chars, &mut k).is_some() {
                i = k;
            } else {
                out.push_str("\\vspace");
                i = j;
            }
            continue;
        }
        if let Some(sym) = latex_symbol(&name) {
            out.push_str(sym);
            i = j;
            continue;
        }

        out.push('\\');
        out.push_str(&name);
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latex_translator_symbols_and_structures() {
        assert_eq!(translate_latex(r"\alpha + \beta = \gamma"), "α + β = γ");
        assert_eq!(translate_latex(r"\sum_{i=1}^{n} x_i"), "∑_i=1^n x_i");
        assert_eq!(translate_latex(r"\frac{a}{b}"), "(a)/(b)");
        assert_eq!(translate_latex(r"\sqrt{x^2}"), "√(x^2)");
        assert_eq!(translate_latex(r"x \in \mathbb{R}"), "x ∈ ℝ");
        assert_eq!(
            translate_latex(r"\left( \frac{1}{2} \right)"),
            "( (1)/(2) )"
        );
        assert_eq!(translate_latex(r"a \leq b \neq c"), "a ≤ b ≠ c");
        assert_eq!(translate_latex(r"\text{eff} = P"), "eff = P");
        assert_eq!(translate_latex(r"\infty \to \pm 1"), "∞ → ± 1");

        assert_eq!(translate_latex(r"\$100 \& 50\%"), "$100 & 50%");
        assert_eq!(translate_latex(r"{a}_{ij}"), "a_ij");

        assert_eq!(translate_latex(r"a \qquad b \quad c"), "a      b    c");
        assert_eq!(translate_latex(r"a\!b"), "ab");
        assert_eq!(translate_latex(r"a\hspace{1cm}b"), "a b");
    }

    #[test]
    fn bare_latex_translates_only_known_commands() {
        assert_eq!(
            translate_bare_latex_chunk(r"Av \approx -gm R_C"),
            "Av ≈ -gm R_C"
        );
        assert_eq!(translate_bare_latex_chunk(r"T = A_v \beta"), "T = A_v β");
        assert_eq!(
            translate_bare_latex_chunk(r"\frac{C1}{C1 + C2}"),
            "(C1)/(C1 + C2)"
        );
        assert_eq!(translate_bare_latex_chunk(r"x \in \mathbb{R}"), "x ∈ ℝ");

        assert_eq!(translate_bare_latex_chunk(r"C:\new\temp"), r"C:\new\temp");
        assert_eq!(translate_bare_latex_chunk(r"R_C and {a}"), "R_C and {a}");
        assert_eq!(
            translate_bare_latex_chunk(r"use \tools daily"),
            r"use \tools daily"
        );
        assert_eq!(translate_bare_latex_chunk("costs $100"), "costs $100");
        assert_eq!(translate_bare_latex_chunk(r"C:\\share"), r"C:\\share");
        assert_eq!(translate_bare_latex_chunk(r"a \qquad b"), "a      b");
        assert_eq!(translate_bare_latex_chunk(r"a\!b"), "ab");
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn art(content: &str) -> Vec<String> {
        render_display_math(content, 60).expect("renders")
    }

    #[test]
    fn frac_simple() {
        assert_eq!(art(r"\frac{a}{b}"), vec!["a", "─", "b"]);
    }

    #[test]
    fn frac_nested() {
        assert_eq!(
            art(r"\frac{1}{1+\frac{1}{x}}"),
            vec!["1", "─────", "    1", "1 + ─", "    x"]
        );
    }

    #[test]
    fn sum_with_limits_stacks() {
        assert_eq!(art(r"\sum_{i=0}^{n} x_i"), vec!["  n", "  ∑  xᵢ", "i = 0"]);
    }

    #[test]
    fn integral_with_limits() {
        let rows = art(r"\int_0^\infty e^{-x^2} dx");
        assert_eq!(rows.len(), 3);
        assert!(rows[0].contains('∞'), "{rows:?}");
        assert!(rows[1].starts_with('∫'), "{rows:?}");
        assert!(rows[1].contains("e⁻ˣ²"), "{rows:?}");
        assert_eq!(rows[2].trim(), "0");
    }

    #[test]
    fn pmatrix_2x2() {
        assert_eq!(
            art(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}"),
            vec!["⎛a  b⎞", "⎝c  d⎠"]
        );
    }

    #[test]
    fn bmatrix_3x3() {
        assert_eq!(
            art(r"\begin{bmatrix} 1 & 2 & 3 \\ 4 & 5 & 6 \\ 7 & 8 & 9 \end{bmatrix}"),
            vec!["⎡1  2  3⎤", "⎢4  5  6⎥", "⎣7  8  9⎦"]
        );
    }

    #[test]
    fn matrix_with_fraction_cells() {
        // Single-row cells baseline-align with the adjacent fraction rules.
        assert_eq!(
            art(r"\begin{pmatrix} \frac{1}{2} & 0 \\ 0 & \frac{3}{4} \end{pmatrix}"),
            vec!["⎛1   ⎞", "⎜─  0⎟", "⎜2   ⎟", "⎜   3⎟", "⎜0  ─⎟", "⎝   4⎠"]
        );
    }

    #[test]
    fn sqrt_simple() {
        assert_eq!(art(r"\sqrt{x^2 + y^2}"), vec![" ───────", "√x² + y²"]);
    }

    #[test]
    fn grown_parens() {
        assert_eq!(art(r"\left(\frac{a}{b}\right)"), vec!["⎛a⎞", "⎜─⎟", "⎝b⎠"]);
    }

    #[test]
    fn binom_is_two_row_parens() {
        assert_eq!(art(r"\binom{n}{k}"), vec!["⎛n⎞", "⎝k⎠"]);
    }

    #[test]
    fn lim_takes_below_limit() {
        let rows = art(r"\lim_{x \to 0} f(x)");
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows[0].contains("lim"), "{rows:?}");
        assert!(rows[0].contains("f(x)"), "{rows:?}");
        assert!(rows[1].contains("x → 0"), "{rows:?}");
    }

    #[test]
    fn overline_spans_body() {
        assert_eq!(art(r"\overline{x + y}"), vec!["─────", "x + y"]);
    }

    #[test]
    fn aligned_rows_split() {
        assert_eq!(
            art(r"\begin{aligned} a &= b \\ c &= d \end{aligned}"),
            vec!["a = b", "c = d"]
        );
    }

    #[test]
    fn cases_brace() {
        let rows = art(r"\begin{cases} x & x > 0 \\ -x & x \le 0 \end{cases}");
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows[0].starts_with('⎧'), "{rows:?}");
        assert!(rows[1].starts_with('⎩'), "{rows:?}");
    }

    #[test]
    fn text_mode_is_literal() {
        assert_eq!(art(r"\text{hello } x"), vec!["hello x"]);
    }

    #[test]
    fn accents_stack_mark() {
        assert_eq!(art(r"\hat{x}"), vec!["^", "x"]);
        assert_eq!(art(r"\vec{v}"), vec!["→", "v"]);
    }

    #[test]
    fn primes_append() {
        assert_eq!(art("f''(x)"), vec!["f″(x)"]);
    }

    #[test]
    fn font_commands_map() {
        assert_eq!(art(r"\mathbf{R}"), vec!["𝐑"]);
        assert_eq!(art(r"\mathbb{R}"), vec!["ℝ"]);
        assert_eq!(art(r"\mathcal{L}"), vec!["ℒ"]);
    }

    #[test]
    fn unknown_env_flows_through() {
        assert_eq!(art(r"\begin{foo} a + b \end{foo}"), vec!["a + b"]);
    }

    #[test]
    fn wide_art_falls_back() {
        assert!(render_display_math(
            r"\frac{aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}{b}",
            40
        )
        .is_none());
    }

    #[test]
    fn empty_is_none() {
        assert!(render_display_math("", 60).is_none());
        assert!(render_display_math("   ", 60).is_none());
    }

    #[test]
    fn garbage_never_panics() {
        for bad in [
            r"\frac{a}",
            r"\frac{a}{b",
            r"x^{2",
            r"\sqrt{",
            r"\left(\frac{a}{b}",
            r"\begin{pmatrix} a & b",
            r"\unknowncmd quux",
            r"{{{",
            r"x_",
            r"100 & 200",
            r"\\",
        ] {
            let _ = render_display_math(bad, 60);
            let _ = render_inline_math(bad);
        }
    }

    #[test]
    fn inline_stays_single_line() {
        assert_eq!(render_inline_math("x^2"), "x²");
        assert_eq!(render_inline_math("a_n"), "aₙ");
        assert_eq!(render_inline_math("x_i^2"), "xᵢ²");
        assert_eq!(render_inline_math("E = mc^2"), "E = mc²");
        assert_eq!(render_inline_math(r"\alpha + \beta"), "α + β");
        // Multi-row structures fall back to the flat legacy form.
        assert_eq!(render_inline_math(r"\frac{a}{b}"), "(a)/(b)");
        assert_eq!(render_inline_math(r"\binom{n}{k}"), "(n choose k)");
        assert_eq!(render_inline_math(r"\unknowncmd{x}"), "x");
    }
}
