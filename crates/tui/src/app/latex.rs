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
