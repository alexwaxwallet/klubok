//! Выражения для `#if` и `#math`: `$hp < 50 && "$tank" != ""`, `($a + 2) * 3`.
//!
//! Переменные уже подставлены до вызова. Значения сравниваются как числа,
//! если обе стороны — числа, иначе как строки. Строки — в кавычках или голые слова.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Num(f64),
    Str(String),
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Value::Num(n) => *n != 0.0,
            Value::Str(s) => !s.is_empty() && s != "0",
        }
    }

    fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Str(s) => s.trim().parse().ok(),
        }
    }

    /// Текстовый вид: целые числа без «.0».
    pub fn to_text(&self) -> String {
        match self {
            Value::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
            Value::Num(n) => n.to_string(),
            Value::Str(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Str(String),
    Op(&'static str),
    Open,
    Close,
}

const OPERATORS: [&str; 16] = [
    "&&", "||", "==", "!=", "<=", ">=", "=", "<", ">", "+", "-", "*", "/", "%", "!", "^",
];

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            tokens.push(Token::Open);
            i += 1;
        } else if c == ')' {
            tokens.push(Token::Close);
            i += 1;
        } else if c == '"' || c == '\'' {
            let mut s = String::new();
            i += 1;
            while i < chars.len() && chars[i] != c {
                s.push(chars[i]);
                i += 1;
            }
            i += 1; // закрывающая кавычка
            tokens.push(Token::Str(s));
        } else if let Some(op) = OPERATORS.iter().find(|op| {
            let len = op.chars().count();
            i + len <= chars.len() && chars[i..i + len].iter().collect::<String>() == **op
        }) {
            tokens.push(Token::Op(op));
            i += op.chars().count();
        } else {
            // Число или голое слово — до пробела, скобки или оператора.
            let mut s = String::new();
            while i < chars.len()
                && !chars[i].is_whitespace()
                && !"()&|=!<>+-*/%^\"'".contains(chars[i])
            {
                s.push(chars[i]);
                i += 1;
            }
            if s.is_empty() {
                return Err(format!("непонятный символ «{c}»"));
            }
            tokens.push(match s.parse::<f64>() {
                Ok(n) => Token::Num(n),
                Err(_) => Token::Str(s),
            });
        }
    }
    Ok(tokens)
}

/// Вычисляет выражение.
pub fn eval(text: &str) -> Result<Value, String> {
    let tokens = tokenize(text)?;
    if tokens.is_empty() {
        return Ok(Value::Str(String::new()));
    }
    let mut parser = Parser { tokens, pos: 0 };
    let value = parser.or()?;
    if parser.pos < parser.tokens.len() {
        return Err(format!("лишнее в выражении «{text}»"));
    }
    Ok(value)
}

/// Разбор с приоритетами: `||` < `&&` < сравнения < `+ -` < `* / %` < унарные.
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek_op(&self) -> Option<&'static str> {
        match self.tokens.get(self.pos) {
            Some(Token::Op(op)) => Some(op),
            _ => None,
        }
    }

    fn or(&mut self) -> Result<Value, String> {
        let mut left = self.and()?;
        while self.peek_op() == Some("||") {
            self.pos += 1;
            let right = self.and()?;
            left = bool_value(left.truthy() || right.truthy());
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Value, String> {
        let mut left = self.compare()?;
        while self.peek_op() == Some("&&") {
            self.pos += 1;
            let right = self.compare()?;
            left = bool_value(left.truthy() && right.truthy());
        }
        Ok(left)
    }

    fn compare(&mut self) -> Result<Value, String> {
        let left = self.sum()?;
        let Some(op) = self.peek_op() else {
            return Ok(left);
        };
        if !["==", "=", "!=", "<", ">", "<=", ">="].contains(&op) {
            return Ok(left);
        }
        self.pos += 1;
        let right = self.sum()?;
        let ordering = match (left.as_num(), right.as_num()) {
            (Some(a), Some(b)) => a.partial_cmp(&b),
            _ => Some(left.to_text().cmp(&right.to_text())),
        };
        let Some(ordering) = ordering else {
            return Ok(bool_value(false));
        };
        use std::cmp::Ordering::*;
        Ok(bool_value(match op {
            "==" | "=" => ordering == Equal,
            "!=" => ordering != Equal,
            "<" => ordering == Less,
            ">" => ordering == Greater,
            "<=" => ordering != Greater,
            _ => ordering != Less,
        }))
    }

    fn sum(&mut self) -> Result<Value, String> {
        let mut left = self.product()?;
        while let Some(op @ ("+" | "-")) = self.peek_op() {
            self.pos += 1;
            let right = self.product()?;
            left = arith(&left, &right, op)?;
        }
        Ok(left)
    }

    fn product(&mut self) -> Result<Value, String> {
        let mut left = self.unary()?;
        while let Some(op @ ("*" | "/" | "%")) = self.peek_op() {
            self.pos += 1;
            let right = self.unary()?;
            left = arith(&left, &right, op)?;
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Value, String> {
        match self.peek_op() {
            Some("!") => {
                self.pos += 1;
                Ok(bool_value(!self.unary()?.truthy()))
            }
            Some("-") => {
                self.pos += 1;
                let v = self.unary()?;
                let n = v.as_num().ok_or("минус перед не-числом")?;
                Ok(Value::Num(-n))
            }
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Result<Value, String> {
        let token = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        match token {
            Some(Token::Num(n)) => Ok(Value::Num(n)),
            Some(Token::Str(s)) => Ok(Value::Str(s)),
            Some(Token::Open) => {
                let v = self.or()?;
                if self.tokens.get(self.pos) != Some(&Token::Close) {
                    return Err("не хватает «)»".into());
                }
                self.pos += 1;
                Ok(v)
            }
            // Пустая сторона сравнения: `$tank == ` при пустой переменной.
            Some(Token::Close) | Some(Token::Op(_)) | None => {
                self.pos -= 1;
                Ok(Value::Str(String::new()))
            }
        }
    }
}

fn bool_value(b: bool) -> Value {
    Value::Num(if b { 1.0 } else { 0.0 })
}

fn arith(a: &Value, b: &Value, op: &str) -> Result<Value, String> {
    let (Some(x), Some(y)) = (a.as_num(), b.as_num()) else {
        if op == "+" {
            return Ok(Value::Str(a.to_text() + &b.to_text()));
        }
        return Err(format!(
            "«{}» {op} «{}»: нужны числа",
            a.to_text(),
            b.to_text()
        ));
    };
    Ok(Value::Num(match op {
        "+" => x + y,
        "-" => x - y,
        "*" => x * y,
        // Как в JMC: целочисленное деление, если оба числа целые.
        "/" if y == 0.0 => return Err("деление на ноль".into()),
        "/" if x.fract() == 0.0 && y.fract() == 0.0 => (x / y).trunc(),
        "/" => x / y,
        _ if y == 0.0 => return Err("деление на ноль".into()),
        _ => x % y,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn truth(s: &str) -> bool {
        eval(s).unwrap().truthy()
    }

    #[test]
    fn сравнения() {
        assert!(truth("1 == 1"));
        assert!(!truth("1 == 0"));
        assert!(truth("10 > 9")); // числа, а не строки
        assert!(truth("\"щит\" != \"0\""));
        assert!(truth("щит != 0"));
        assert!(truth("Вася == Вася && 2 >= 2"));
        assert!(truth("0 || 1"));
        assert!(!truth("!(1)"));
        assert!(truth(" == ")); // две пустые строки равны
    }

    #[test]
    fn арифметика() {
        assert_eq!(eval("(2 + 3) * 4").unwrap().to_text(), "20");
        assert_eq!(eval("7 / 2").unwrap().to_text(), "3");
        assert_eq!(eval("-5 + 10 % 3").unwrap().to_text(), "-4");
        assert!(eval("1 / 0").is_err());
    }
}
