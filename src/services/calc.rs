//! Launcher calculator (`=2+2`, `=sqrt(16)*pi`): a small recursive-descent
//! evaluator, so no expression engine or process is needed per keystroke.
//!
//!   expr    := term (('+' | '-') term)*
//!   term    := unary (('*' | '/' | '%' | implicit) unary)*
//!   unary   := ('-' | '+') unary | power
//!   power   := postfix ('^' unary)?          (right-associative)
//!   postfix := primary '!'*
//!   primary := number | '(' expr ')' | ident ['(' expr ')']
//!
//! Both '.' and ',' are decimal separators (Spanish keyboards type "1,5");
//! functions take a single argument, so ',' is never ambiguous. Implicit
//! multiplication covers "2pi", "3(1+1)" and "90deg".

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum CalcError {
    Empty,
    Syntax,
    UnknownName(String),
    /// Division by zero, sqrt(-1), overflow...
    Undefined,
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CalcError::Empty => f.write_str("empty expression"),
            CalcError::Syntax => f.write_str("syntax error"),
            CalcError::UnknownName(n) => write!(f, "unknown name '{n}'"),
            CalcError::Undefined => f.write_str("undefined result"),
        }
    }
}

/// Evaluates `expr`; only finite results are returned.
pub fn eval(expr: &str) -> Result<f64, CalcError> {
    let tokens = lex(expr)?;
    if tokens.is_empty() {
        return Err(CalcError::Empty);
    }
    let mut p = Parser { tokens, pos: 0 };
    let v = p.expr()?;
    if p.pos != p.tokens.len() {
        return Err(CalcError::Syntax);
    }
    if v.is_finite() { Ok(v) } else { Err(CalcError::Undefined) }
}

/// Shows a result with up to 12 significant digits, no trailing zeros, the
/// locale's decimal separator and scientific notation for extreme values.
pub fn format(v: f64, decimal_comma: bool) -> String {
    let v = if v == 0.0 { 0.0 } else { v }; // no "-0"
    let abs = v.abs();
    let s = if abs != 0.0 && !(1e-9..1e15).contains(&abs) {
        let s = format!("{v:.10e}");
        let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
        format!("{}e{exp}", trim_zeros(mantissa))
    } else if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        // 12 significant digits overall hides binary noise (0.1+0.2).
        let decimals = (11 - abs.log10().floor() as i32).clamp(0, 15) as usize;
        trim_zeros(&format!("{v:.decimals$}")).to_owned()
    };
    if decimal_comma { s.replace('.', ",") } else { s }
}

fn trim_zeros(s: &str) -> &str {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    Open,
    Close,
}

fn lex(s: &str) -> Result<Vec<Tok>, CalcError> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '0'..='9' | '.' | ',' => {
                let start = i;
                let mut seen_sep = false;
                while i < chars.len() && (chars[i].is_ascii_digit() || (matches!(chars[i], '.' | ',') && !seen_sep)) {
                    seen_sep |= matches!(chars[i], '.' | ',');
                    i += 1;
                }
                // Exponent only when digits follow ("2e" is 2·e, "2e3" is 2000).
                if i < chars.len() && matches!(chars[i], 'e' | 'E') {
                    let mut j = i + 1;
                    if j < chars.len() && matches!(chars[j], '+' | '-') {
                        j += 1;
                    }
                    if j < chars.len() && chars[j].is_ascii_digit() {
                        i = j;
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
                let text: String = chars[start..i].iter().map(|&c| if c == ',' { '.' } else { c }).collect();
                out.push(Tok::Num(text.parse().map_err(|_| CalcError::Syntax)?));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                out.push(Tok::Ident(chars[start..i].iter().collect::<String>().to_lowercase()));
            }
            '(' | '[' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' | ']' => {
                out.push(Tok::Close);
                i += 1;
            }
            '*' if chars.get(i + 1) == Some(&'*') => {
                out.push(Tok::Op('^'));
                i += 2;
            }
            '+' | '-' | '*' | '/' | '%' | '^' | '!' => {
                out.push(Tok::Op(c));
                i += 1;
            }
            '×' | '·' => {
                out.push(Tok::Op('*'));
                i += 1;
            }
            '÷' => {
                out.push(Tok::Op('/'));
                i += 1;
            }
            '−' => {
                out.push(Tok::Op('-'));
                i += 1;
            }
            _ => return Err(CalcError::Syntax),
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn eat_op(&mut self, ops: &[char]) -> Option<char> {
        match self.peek() {
            Some(Tok::Op(c)) if ops.contains(c) => {
                let c = *c;
                self.pos += 1;
                Some(c)
            }
            _ => None,
        }
    }

    fn expr(&mut self) -> Result<f64, CalcError> {
        let mut v = self.term()?;
        while let Some(op) = self.eat_op(&['+', '-']) {
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        Ok(v)
    }

    fn term(&mut self) -> Result<f64, CalcError> {
        let mut v = self.unary()?;
        loop {
            if let Some(op) = self.eat_op(&['*', '/', '%']) {
                let r = self.unary()?;
                v = match op {
                    '*' => v * r,
                    '/' if r == 0.0 => return Err(CalcError::Undefined),
                    '/' => v / r,
                    _ if r == 0.0 => return Err(CalcError::Undefined),
                    _ => v.rem_euclid(r),
                };
            } else if matches!(self.peek(), Some(Tok::Open | Tok::Ident(_))) {
                // Implicit multiplication: 2pi, 3(4), (1+1)(2+2), 90deg. Never
                // between two numbers: "1.2.3" or "2 3" are typos, not products.
                v *= self.power()?;
            } else {
                return Ok(v);
            }
        }
    }

    fn unary(&mut self) -> Result<f64, CalcError> {
        match self.eat_op(&['-', '+']) {
            Some('-') => Ok(-self.unary()?),
            Some(_) => self.unary(),
            None => self.power(),
        }
    }

    fn power(&mut self) -> Result<f64, CalcError> {
        let base = self.postfix()?;
        if self.eat_op(&['^']).is_some() {
            let exp = self.unary()?;
            return Ok(base.powf(exp));
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<f64, CalcError> {
        let mut v = self.primary()?;
        while self.eat_op(&['!']).is_some() {
            v = factorial(v)?;
        }
        Ok(v)
    }

    fn primary(&mut self) -> Result<f64, CalcError> {
        match self.tokens.get(self.pos).cloned() {
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(n)
            }
            Some(Tok::Open) => {
                self.pos += 1;
                let v = self.expr()?;
                // A missing ')' at the very end is forgiven while typing.
                match self.peek() {
                    Some(Tok::Close) => self.pos += 1,
                    None => {}
                    _ => return Err(CalcError::Syntax),
                }
                Ok(v)
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                if let Some(c) = constant(&name) {
                    return Ok(c);
                }
                let f = function(&name).ok_or(CalcError::UnknownName(name))?;
                // Functions bind to a parenthesised group or the next factor (sqrt 16).
                let arg = self.postfix()?;
                let v = f(arg);
                if v.is_nan() { Err(CalcError::Undefined) } else { Ok(v) }
            }
            _ => Err(CalcError::Syntax),
        }
    }
}

fn constant(name: &str) -> Option<f64> {
    use std::f64::consts;
    Some(match name {
        "pi" | "π" => consts::PI,
        "tau" | "τ" => consts::TAU,
        "e" => consts::E,
        "phi" | "φ" => 1.618_033_988_749_895,
        "deg" | "°" => consts::PI / 180.0,
        _ => return None,
    })
}

fn function(name: &str) -> Option<fn(f64) -> f64> {
    Some(match name {
        "sqrt" | "raiz" | "raíz" => f64::sqrt,
        "cbrt" => f64::cbrt,
        "abs" => f64::abs,
        "sin" | "sen" => f64::sin,
        "cos" => f64::cos,
        "tan" => f64::tan,
        "asin" | "arcsin" => f64::asin,
        "acos" | "arccos" => f64::acos,
        "atan" | "arctan" => f64::atan,
        "ln" => f64::ln,
        "log" | "log10" => f64::log10,
        "log2" => f64::log2,
        "exp" => f64::exp,
        "floor" => f64::floor,
        "ceil" => f64::ceil,
        "round" => f64::round,
        _ => return None,
    })
}

fn factorial(v: f64) -> Result<f64, CalcError> {
    if v < 0.0 || v.fract() != 0.0 || v > 170.0 {
        return Err(CalcError::Undefined);
    }
    Ok((1..=v as u32).fold(1.0, |acc, n| acc * n as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f64 {
        eval(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(ev("2+2"), 4.0);
        assert_eq!(ev("2+3*4"), 14.0);
        assert_eq!(ev("(2+3)*4"), 20.0);
        assert_eq!(ev("2^3^2"), 512.0);
        assert_eq!(ev("2**10"), 1024.0);
        assert_eq!(ev("-2^2"), -4.0);
        assert_eq!(ev("10-4-3"), 3.0);
        assert_eq!(ev("100/10/5"), 2.0);
        assert_eq!(ev("7 % 3"), 1.0);
        assert_eq!(ev("--3"), 3.0);
    }

    #[test]
    fn decimals_with_point_or_comma() {
        assert_eq!(ev("1,5*2"), 3.0);
        assert_eq!(ev("1.5*2"), 3.0);
        assert_eq!(ev(".5+,5"), 1.0);
        assert_eq!(ev("1e3"), 1000.0);
        assert_eq!(ev("2.5E-1"), 0.25);
    }

    #[test]
    fn functions_constants_and_implicit_multiplication() {
        assert_eq!(ev("sqrt(16)"), 4.0);
        assert_eq!(ev("sqrt 16"), 4.0);
        assert!((ev("2pi") - std::f64::consts::TAU).abs() < 1e-12);
        assert!((ev("sin(90deg)") - 1.0).abs() < 1e-12);
        assert!((ev("2e") - 2.0 * std::f64::consts::E).abs() < 1e-12);
        assert_eq!(ev("3(1+1)"), 6.0);
        assert_eq!(ev("(1+1)(2+2)"), 8.0);
        assert_eq!(ev("5!"), 120.0);
        assert_eq!(ev("log(1000)"), 3.0);
        assert_eq!(ev("2×3÷4"), 1.5);
        assert_eq!(ev("raiz(9)"), 3.0);
        // Unclosed parenthesis at the end while typing.
        assert_eq!(ev("2*(3+4"), 14.0);
    }

    #[test]
    fn errors() {
        assert_eq!(eval(""), Err(CalcError::Empty));
        assert_eq!(eval("   "), Err(CalcError::Empty));
        assert_eq!(eval("1/0"), Err(CalcError::Undefined));
        assert_eq!(eval("5%0"), Err(CalcError::Undefined));
        assert_eq!(eval("sqrt(-1)"), Err(CalcError::Undefined));
        assert_eq!(eval("(-1)!"), Err(CalcError::Undefined));
        assert_eq!(eval("2+"), Err(CalcError::Syntax));
        assert_eq!(eval("2)"), Err(CalcError::Syntax));
        assert_eq!(eval("1.2.3"), Err(CalcError::Syntax));
        assert_eq!(eval("2 3"), Err(CalcError::Syntax));
        assert_eq!(eval("foo(2)"), Err(CalcError::UnknownName("foo".into())));
        assert_eq!(eval("10^400"), Err(CalcError::Undefined));
        assert_eq!(eval("2 $ 3"), Err(CalcError::Syntax));
    }

    #[test]
    fn formats_results() {
        assert_eq!(format(4.0, false), "4");
        assert_eq!(format(0.1 + 0.2, false), "0.3");
        assert_eq!(format(1.5, true), "1,5");
        assert_eq!(format(-0.0, false), "0");
        assert_eq!(format(1.0 / 3.0, false), "0.333333333333");
        assert_eq!(format(123456.789, false), "123456.789");
        assert_eq!(format(1e20, false), "1e20");
        assert_eq!(format(1.5e-12, false), "1.5e-12");
        assert_eq!(format(-2.5e16, true), "-2,5e16");
    }
}
