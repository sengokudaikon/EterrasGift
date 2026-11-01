//! Reads the plural rule in a catalog's header, so a count picks the form its language wants.

/// A gettext plural expression: C's integer operators over `n`, as `Plural-Forms: plural=…` writes them.
#[derive(Debug)]
pub(crate) struct Rule(Expr);

#[derive(Debug)]
enum Expr {
    N,
    Number(u64),
    Not(Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
    Choose(Box<Expr>, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl Rule {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut parser = Parser {
            rest: text.trim().trim_end_matches(';'),
        };
        let expr = parser.choose()?;
        parser.skip();
        if parser.rest.is_empty() {
            Ok(Self(expr))
        } else {
            Err(format!("unexpected {:?}", parser.rest))
        }
    }

    pub(crate) fn form(&self, n: u64) -> usize {
        usize::try_from(eval(&self.0, n)).unwrap_or(usize::MAX)
    }
}

fn eval(expr: &Expr, n: u64) -> u64 {
    match expr {
        Expr::N => n,
        Expr::Number(value) => *value,
        Expr::Not(inner) => u64::from(eval(inner, n) == 0),
        Expr::Choose(test, yes, no) => eval(if eval(test, n) != 0 { yes } else { no }, n),
        Expr::Binary(op, left, right) => {
            let (a, b) = (eval(left, n), eval(right, n));
            match op {
                Op::Or => u64::from(a != 0 || b != 0),
                Op::And => u64::from(a != 0 && b != 0),
                Op::Eq => u64::from(a == b),
                Op::Ne => u64::from(a != b),
                Op::Lt => u64::from(a < b),
                Op::Le => u64::from(a <= b),
                Op::Gt => u64::from(a > b),
                Op::Ge => u64::from(a >= b),
                Op::Add => a.wrapping_add(b),
                Op::Sub => a.wrapping_sub(b),
                Op::Mul => a.wrapping_mul(b),
                Op::Div => a.checked_div(b).unwrap_or(0),
                Op::Rem => a.checked_rem(b).unwrap_or(0),
            }
        }
    }
}

/// Recursive descent in C's precedence: `?:`, `||`, `&&`, equality, comparison, `+ -`, `* / %`, `!`.
struct Parser<'a> {
    rest: &'a str,
}

impl Parser<'_> {
    fn skip(&mut self) {
        self.rest = self.rest.trim_start();
    }

    fn eat(&mut self, token: &str) -> bool {
        self.skip();
        // `<` must not take the start of `<=`, nor `!` of `!=`.
        let longer = matches!(token, "<" | ">" | "!") && self.rest.get(token.len()..).is_some_and(|after| after.starts_with('='));
        if self.rest.starts_with(token) && !longer {
            self.rest = &self.rest[token.len()..];
            true
        } else {
            false
        }
    }

    fn choose(&mut self) -> Result<Expr, String> {
        let test = self.binary(0)?;
        if !self.eat("?") {
            return Ok(test);
        }
        let yes = self.choose()?;
        if !self.eat(":") {
            return Err(format!("expected ':' at {:?}", self.rest));
        }
        Ok(Expr::Choose(Box::new(test), Box::new(yes), Box::new(self.choose()?)))
    }

    fn binary(&mut self, level: usize) -> Result<Expr, String> {
        const LEVELS: [&[(&str, Op)]; 6] = [
            &[("||", Op::Or)],
            &[("&&", Op::And)],
            &[("==", Op::Eq), ("!=", Op::Ne)],
            &[("<=", Op::Le), (">=", Op::Ge), ("<", Op::Lt), (">", Op::Gt)],
            &[("+", Op::Add), ("-", Op::Sub)],
            &[("*", Op::Mul), ("/", Op::Div), ("%", Op::Rem)],
        ];
        let Some(ops) = LEVELS.get(level) else {
            return self.unary();
        };
        let mut left = self.binary(level + 1)?;
        while let Some((_, op)) = ops.iter().find(|(token, _)| self.eat(token)) {
            left = Expr::Binary(*op, Box::new(left), Box::new(self.binary(level + 1)?));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.eat("(") {
            let inner = self.choose()?;
            return if self.eat(")") { Ok(inner) } else { Err(format!("expected ')' at {:?}", self.rest)) };
        }
        if self.eat("n") {
            return Ok(Expr::N);
        }
        let digits = self.rest.len() - self.rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let value = self.rest[..digits].parse().map_err(|_| format!("expected a value at {:?}", self.rest))?;
        self.rest = &self.rest[digits..];
        Ok(Expr::Number(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_rules_pick_each_languages_forms() {
        let forms = |rule: &str, counts: &[u64]| {
            let rule = Rule::parse(rule).unwrap();
            counts.iter().map(|n| rule.form(*n)).collect::<Vec<_>>()
        };
        assert_eq!(forms("(n != 1);", &[0, 1, 2]), [1, 0, 1]);
        assert_eq!(forms("0", &[0, 1, 5]), [0, 0, 0]);
        let polish = "(n==1 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2)";
        assert_eq!(forms(polish, &[1, 2, 4, 5, 12, 22, 25, 112]), [0, 1, 1, 2, 2, 1, 2, 2]);
        let arabic = "n==0 ? 0 : n==1 ? 1 : n==2 ? 2 : n%100>=3 && n%100<=10 ? 3 : n%100>=11 ? 4 : 5";
        assert_eq!(forms(arabic, &[0, 1, 2, 3, 10, 11, 99, 100, 102]), [0, 1, 2, 3, 3, 4, 4, 5, 5]);
        assert_eq!(forms("!(n < 2)", &[1, 2]), [0, 1]);
        for broken in ["n ==", "(n != 1", "n ? 1", "x"] {
            assert!(Rule::parse(broken).is_err(), "{broken}");
        }
    }
}
