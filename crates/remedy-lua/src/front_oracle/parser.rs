//! The recursive parser, kept as the oracle the iterative one is checked against
//! (feature `front-oracle`). Unchanged but for a move out of a field that now has a `Drop`.
//!
//! The parser: tokens to a syntax tree, Lua 5.4's grammar plus RemedyLua's additions when
//! the dialect allows them.

use crate::Dialect;
use crate::ast::*;
use crate::diag::{Diagnostic, Pos};
use crate::lexer::{InterpPart, Lexer, Tok, Token};

/// How deeply expressions and blocks may nest (Lua's own limit is the same).
pub(crate) const MAX_DEPTH: usize = 200;
/// Longest chain of left-associative operators (`a + b + c ...`): compiled in a loop, but
/// the tree is still dropped recursively.
const MAX_CHAIN: usize = 1000;

pub(crate) struct Parser<'a> {
    toks: Vec<Token>,
    /// The text the tokens came from (to quote them in messages).
    src: &'a [u8],
    i: usize,
    chunk: &'a str,
    dialect: Dialect,
    depth: usize,
    /// Per function being parsed: whether it is variadic.
    vararg: Vec<bool>,
    /// Where the native stack was when parsing began, and how much it may use.
    stack_base: usize,
    stack_bytes: usize,
}

type PResult<T> = Result<T, Diagnostic>;

/// Parses a whole chunk.
pub(crate) fn parse(src: &[u8], chunk: &str, dialect: Dialect) -> PResult<Block> {
    let toks = Lexer::new(src, chunk, dialect).tokens()?;
    let mut p = Parser {
        toks,
        src,
        i: 0,
        chunk,
        dialect,
        depth: 0,
        vararg: vec![true],
        stack_base: crate::stack_pos(),
        stack_bytes: DEFAULT_STACK,
    };
    let b = p.block()?;
    if !p.check(&Tok::Eof) {
        return Err(p.error_near("<eof> expected"));
    }
    Ok(b)
}

/// Native stack the front end may use by default.
pub(crate) const DEFAULT_STACK: usize = 512 << 10;

impl<'a> Parser<'a> {
    fn rl(&self) -> bool {
        self.dialect == Dialect::RemedyLua
    }

    fn tok(&self) -> &Tok {
        self.toks.get(self.i).map(|t| &t.tok).unwrap_or(&Tok::Eof)
    }

    fn tok_at(&self, n: usize) -> &Tok {
        self.toks.get(self.i + n).map(|t| &t.tok).unwrap_or(&Tok::Eof)
    }

    /// Where the current token ends (just past it).
    fn tok_end(&self) -> Pos {
        self.toks.get(self.i).map_or_else(|| self.pos(), |t| t.end)
    }

    /// Where the last token read ends (just past it): the end of what was just parsed.
    fn last_end(&self) -> Pos {
        match self.i.checked_sub(1).and_then(|j| self.toks.get(j)) {
            Some(t) => t.end,
            None => self.pos(),
        }
    }

    fn pos(&self) -> Pos {
        self.toks.get(self.i).or(self.toks.last()).map(|t| t.pos).unwrap_or_default()
    }

    fn advance(&mut self) -> Tok {
        let t = self.tok().clone();
        if self.i < self.toks.len() {
            self.i += 1;
        }
        t
    }

    fn check(&self, t: &Tok) -> bool {
        self.tok() == t
    }

    fn accept(&mut self, t: &Tok) -> bool {
        if self.check(t) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// The current token as Lua's messages quote it: as written, or `<eof>`.
    fn near(&self) -> String {
        let Some(t) = self.toks.get(self.i) else { return "<eof>".into() };
        match &t.tok {
            Tok::Eof => "<eof>".into(),
            Tok::Name(_) | Tok::Int(_) | Tok::Num(_) | Tok::Str(_) if t.span.1 > t.span.0 => {
                crate::lexer::near_text(self.src, t)
            }
            other => format!("'{}'", other.describe()),
        }
    }

    /// An error found just after a token was taken (C Lua's `luaK_semerror`): C Lua had
    /// read the next token by then, so a lexical error there comes first, and its message
    /// gives the line where that token ends (S5C-FRONT-7).
    fn semerror(&self, pos: Pos, msg: String) -> Diagnostic {
        if let Some(Token { tok: Tok::Error(d), .. }) = self.toks.get(self.i) {
            return (**d).clone();
        }
        Diagnostic::error(self.chunk, pos, msg).lua_line(self.tok_end().line)
    }

    fn error_near(&self, msg: &str) -> Diagnostic {
        if let Some(Token { tok: Tok::Error(d), .. }) = self.toks.get(self.i) {
            return (**d).clone();
        }
        // About the token; Lua's message gives the line where it ends.
        let end = self.tok_end();
        // A NUL byte is token 0 in C Lua, which its message leaves out.
        let text = if matches!(self.tok(), Tok::Other(0)) { msg.to_owned() } else { format!("{msg} near {}", self.near()) };
        Diagnostic::error(self.chunk, self.pos(), text).to(end).lua_line(end.line)
    }

    fn expect(&mut self, t: &Tok, what: &str) -> PResult<()> {
        if self.accept(t) { Ok(()) } else { Err(self.error_near(&format!("'{what}' expected"))) }
    }

    /// Expects the closing token of a construct opened at `open`.
    fn expect_match(&mut self, t: &Tok, what: &str, opener: &str, open: Pos) -> PResult<()> {
        if self.accept(t) {
            return Ok(());
        }
        if open.line == self.tok_end().line {
            Err(self.error_near(&format!("'{what}' expected")))
        } else {
            Err(self.error_near(&format!("'{what}' expected (to close '{opener}' at line {})", open.line)))
        }
    }

    fn name(&mut self) -> PResult<String> {
        match self.tok().clone() {
            Tok::Name(n) => {
                self.advance();
                Ok(n)
            }
            _ => Err(self.error_near("<name> expected")),
        }
    }

    fn enter(&mut self) -> PResult<()> {
        self.depth += 1;
        // The compiler recurses as deeply later: half the room is left for it.
        if self.depth > MAX_DEPTH || self.stack_base.abs_diff(crate::stack_pos()) > self.stack_bytes {
            return Err(Diagnostic::error(
                self.chunk,
                self.pos(),
                format!("chunk has too many syntax levels (limit is {MAX_DEPTH})"),
            ));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn block_follow(&self, with_until: bool) -> bool {
        match self.tok() {
            Tok::Else | Tok::Elseif | Tok::End | Tok::Eof => true,
            Tok::Until => with_until,
            _ => false,
        }
    }

    fn block(&mut self) -> PResult<Block> {
        self.enter()?;
        let mut stats = Vec::new();
        while !self.block_follow(true) {
            if self.check(&Tok::Return) {
                stats.push(self.ret_stat()?);
                break;
            }
            if let Some(s) = self.statement()? {
                stats.push(s);
            }
        }
        let end = self.pos();
        self.leave();
        Ok(Block { stats, end })
    }

    fn ret_stat(&mut self) -> PResult<Stat> {
        let pos = self.pos();
        self.advance();
        let exprs = if self.block_follow(true) || self.check(&Tok::Semi) { Vec::new() } else { self.exprlist()? };
        self.accept(&Tok::Semi);
        Ok(Stat { kind: StatKind::Return(exprs), pos })
    }

    /// Whether the name at the cursor starts a contextual keyword statement (`continue`,
    /// `type X =`) rather than an expression.
    fn contextual(&self) -> Option<&'static str> {
        if !self.rl() {
            return None;
        }
        let Tok::Name(n) = self.tok() else { return None };
        match n.as_str() {
            "continue" => {
                let next = self.tok_at(1);
                let continues_expr = matches!(
                    next,
                    Tok::Assign
                        | Tok::LParen
                        | Tok::Dot
                        | Tok::LBracket
                        | Tok::Colon
                        | Tok::Comma
                        | Tok::Str(_)
                        | Tok::LBrace
                        | Tok::OpAssign(_)
                );
                (!continues_expr).then_some("continue")
            }
            "type" => {
                (matches!(self.tok_at(1), Tok::Name(_)) && matches!(self.tok_at(2), Tok::Assign)).then_some("type")
            }
            _ => None,
        }
    }

    fn statement(&mut self) -> PResult<Option<Stat>> {
        let pos = self.pos();
        let kind = match self.tok().clone() {
            Tok::Semi => {
                self.advance();
                return Ok(None);
            }
            Tok::If => self.if_stat()?,
            Tok::While => {
                self.advance();
                let cond = self.expr()?;
                self.expect(&Tok::Do, "do")?;
                let body = self.block()?;
                self.expect_match(&Tok::End, "end", "while", pos)?;
                StatKind::While(cond, body)
            }
            Tok::Do => {
                self.advance();
                let b = self.block()?;
                self.expect_match(&Tok::End, "end", "do", pos)?;
                StatKind::Do(b)
            }
            Tok::For => self.for_stat(pos)?,
            Tok::Repeat => {
                self.advance();
                let body = self.block()?;
                self.expect_match(&Tok::Until, "until", "repeat", pos)?;
                let cond = self.expr()?;
                StatKind::Repeat(body, cond)
            }
            Tok::Function => {
                self.advance();
                let mut path = vec![self.name()?];
                let mut method = None;
                while self.accept(&Tok::Dot) {
                    path.push(self.name()?);
                }
                if self.accept(&Tok::Colon) {
                    method = Some(self.name()?);
                }
                let body = self.func_body(method.is_some(), pos)?;
                StatKind::Function { path, method, body: Box::new(body) }
            }
            Tok::Local => {
                self.advance();
                if self.accept(&Tok::Function) {
                    let npos = self.pos();
                    let name = self.name()?;
                    let at = self.pos();
                    let body = self.func_body(false, at)?;
                    StatKind::LocalFunction(LocalName { name, attrib: Attrib::None, ty: None, pos: npos }, Box::new(body))
                } else {
                    self.local_stat()?
                }
            }
            Tok::DColon => {
                self.advance();
                let name = self.name()?;
                self.expect(&Tok::DColon, "::")?;
                StatKind::Label(name)
            }
            Tok::Break => {
                self.advance();
                StatKind::Break
            }
            Tok::Goto => {
                self.advance();
                let line = self.tok_end().line;
                StatKind::Goto(self.name()?, line)
            }
            _ => match self.contextual() {
                Some("continue") => {
                    self.advance();
                    StatKind::Continue
                }
                Some(_) => {
                    self.advance();
                    let name = self.name()?;
                    self.expect(&Tok::Assign, "=")?;
                    let ty = self.ty()?;
                    StatKind::TypeDef(name, ty)
                }
                None => self.expr_stat()?,
            },
        };
        Ok(Some(Stat { kind, pos }))
    }

    fn if_stat(&mut self) -> PResult<StatKind> {
        let open = self.pos();
        self.advance();
        let mut conds = Vec::new();
        let c = self.expr()?;
        self.expect(&Tok::Then, "then")?;
        let b = self.block()?;
        conds.push((c, b));
        let mut els = None;
        loop {
            if self.accept(&Tok::Elseif) {
                let c = self.expr()?;
                self.expect(&Tok::Then, "then")?;
                let b = self.block()?;
                conds.push((c, b));
            } else if self.accept(&Tok::Else) {
                els = Some(self.block()?);
                self.expect_match(&Tok::End, "end", "if", open)?;
                break;
            } else {
                self.expect_match(&Tok::End, "end", "if", open)?;
                break;
            }
        }
        Ok(StatKind::If(conds, els))
    }

    fn for_stat(&mut self, open: Pos) -> PResult<StatKind> {
        self.advance();
        let npos = self.pos();
        let first = self.name()?;
        let first_ty = self.opt_annotation()?;
        let first = LocalName { name: first, attrib: Attrib::None, ty: first_ty, pos: npos };
        if self.accept(&Tok::Assign) {
            let start = self.expr()?;
            self.expect(&Tok::Comma, ",")?;
            let limit = self.expr()?;
            let step = if self.accept(&Tok::Comma) { Some(self.expr()?) } else { None };
            self.expect(&Tok::Do, "do")?;
            let do_line = self.last_end().line;
            let body = self.block()?;
            self.expect_match(&Tok::End, "end", "for", open)?;
            return Ok(StatKind::NumFor(Box::new(ForNum { var: first, start, limit, step, body, do_line })));
        }
        let mut names = vec![first];
        while self.accept(&Tok::Comma) {
            let pos = self.pos();
            let n = self.name()?;
            let ty = self.opt_annotation()?;
            names.push(LocalName { name: n, attrib: Attrib::None, ty, pos });
        }
        if !self.check(&Tok::In) {
            return Err(self.error_near(if names.len() > 1 { "'in' expected" } else { "'=' or 'in' expected" }));
        }
        self.advance();
        let line = self.tok_end().line;
        let exprs = self.exprlist()?;
        self.expect(&Tok::Do, "do")?;
        let do_line = self.last_end().line;
        let body = self.block()?;
        self.expect_match(&Tok::End, "end", "for", open)?;
        Ok(StatKind::GenFor { names, exprs, body, line, do_line })
    }

    fn attrib(&mut self) -> PResult<Attrib> {
        if !self.check(&Tok::Lt) {
            return Ok(Attrib::None);
        }
        self.advance();
        let a = self.name()?;
        self.expect(&Tok::Gt, ">")?;
        match a.as_str() {
            "const" => Ok(Attrib::Const),
            "close" => Ok(Attrib::Close),
            _ => Err(self.semerror(self.pos(), format!("unknown attribute '{a}'"))),
        }
    }

    fn local_stat(&mut self) -> PResult<StatKind> {
        let mut names = Vec::new();
        let mut closes = 0;
        loop {
            let pos = self.pos();
            let name = self.name()?;
            let mut attrib = self.attrib()?;
            let ty = self.opt_annotation()?;
            if attrib == Attrib::None {
                attrib = self.attrib()?;
            }
            if attrib == Attrib::Close {
                closes += 1;
                if closes > 1 {
                    return Err(self.semerror(pos, "multiple to-be-closed variables in local list".into()));
                }
            }
            names.push(LocalName { name, attrib, ty, pos });
            if !self.accept(&Tok::Comma) {
                break;
            }
        }
        let exprs = if self.accept(&Tok::Assign) { self.exprlist()? } else { Vec::new() };
        Ok(StatKind::Local(names, exprs))
    }

    fn expr_stat(&mut self) -> PResult<StatKind> {
        let e = self.suffixed()?;
        if self.check(&Tok::Assign) || self.check(&Tok::Comma) {
            if !matches!(e.kind, ExprKind::Name(_) | ExprKind::Index(..)) {
                return Err(self.error_near("syntax error"));
            }
            let mut targets = vec![e];
            let mut follow = Vec::new();
            crate::ast::note_follow(&targets, &mut follow, self.tok_end().line);
            while self.accept(&Tok::Comma) {
                let t = self.suffixed()?;
                if !matches!(t.kind, ExprKind::Name(_) | ExprKind::Index(..)) {
                    return Err(self.error_near("syntax error"));
                }
                targets.push(t);
                crate::ast::note_follow(&targets, &mut follow, self.tok_end().line);
            }
            self.expect(&Tok::Assign, "=")?;
            let exprs = self.exprlist()?;
            return Ok(StatKind::Assign(targets, exprs, follow));
        }
        if let Tok::OpAssign(op) = *self.tok() {
            if !matches!(e.kind, ExprKind::Name(_) | ExprKind::Index(..)) {
                return Err(Diagnostic::error(self.chunk, e.pos, "syntax error (cannot assign to this expression)"));
            }
            self.advance();
            let v = self.expr()?;
            return Ok(StatKind::OpAssign(Box::new((e, op, v))));
        }
        if !matches!(e.kind, ExprKind::Call(..) | ExprKind::Method(..)) {
            return Err(self.error_near("syntax error"));
        }
        Ok(StatKind::Call(e))
    }

    fn exprlist(&mut self) -> PResult<Vec<Expr>> {
        let mut v = vec![self.expr()?];
        while self.accept(&Tok::Comma) {
            v.push(self.expr()?);
        }
        Ok(v)
    }

    pub(crate) fn expr(&mut self) -> PResult<Expr> {
        self.subexpr(0)
    }

    fn binop(&self) -> Option<BinOp> {
        Some(match self.tok() {
            Tok::Plus => BinOp::Add,
            Tok::Minus => BinOp::Sub,
            Tok::Star => BinOp::Mul,
            Tok::Slash => BinOp::Div,
            Tok::DSlash => BinOp::IDiv,
            Tok::Percent => BinOp::Mod,
            Tok::Caret => BinOp::Pow,
            Tok::Concat => BinOp::Concat,
            Tok::Eq => BinOp::Eq,
            Tok::Ne => BinOp::Ne,
            Tok::Lt => BinOp::Lt,
            Tok::Le => BinOp::Le,
            Tok::Gt => BinOp::Gt,
            Tok::Ge => BinOp::Ge,
            Tok::And => BinOp::And,
            Tok::Or => BinOp::Or,
            Tok::Amp => BinOp::BAnd,
            Tok::Pipe => BinOp::BOr,
            Tok::Tilde => BinOp::BXor,
            Tok::Shl => BinOp::Shl,
            Tok::Shr => BinOp::Shr,
            _ => return None,
        })
    }

    fn subexpr(&mut self, limit: u8) -> PResult<Expr> {
        self.enter()?;
        let pos = self.pos();
        let unop = match self.tok() {
            Tok::Not => Some(UnOp::Not),
            Tok::Minus => Some(UnOp::Neg),
            Tok::Hash => Some(UnOp::Len),
            Tok::Tilde => Some(UnOp::BNot),
            _ => None,
        };
        let mut left = if let Some(op) = unop {
            self.advance();
            let e = self.subexpr(UNARY_PRIORITY)?;
            fold_unary(op, e, pos)
        } else {
            self.simple()?
        };
        // Left-associative chains nest the tree without recursing here: they count too.
        let mut chain = 0;
        while let Some(op) = self.binop() {
            let (l, r) = op.priority();
            if l <= limit {
                break;
            }
            let opos = self.pos();
            self.advance();
            let mut right = self.subexpr(r)?;
            left = if op == BinOp::Concat {
                let mut parts = vec![left];
                let mut pos = opos;
                // (A move out of `right.kind` in the original: `Expr` now has a `Drop`.)
                match &mut right.kind {
                    // The line of the last `..`, as in C Lua.
                    ExprKind::Concat(rest) => {
                        pos = right.pos;
                        parts.append(rest);
                    }
                    _ => parts.push(right),
                }
                Expr { kind: ExprKind::Concat(parts), pos, end: self.last_end() }
            } else {
                chain += 1;
                if chain > MAX_CHAIN {
                    return Err(Diagnostic::error(
                        self.chunk,
                        opos,
                        format!("expression too complex (more than {MAX_CHAIN} operators in a chain)"),
                    ));
                }
                fold_binary(op, left, right, opos)
            };
        }
        self.leave();
        Ok(left)
    }

    fn simple(&mut self) -> PResult<Expr> {
        let pos = self.pos();
        let kind = match self.tok().clone() {
            Tok::Nil => ExprKind::Nil,
            Tok::True => ExprKind::True,
            Tok::False => ExprKind::False,
            Tok::Int(i) => ExprKind::Int(i),
            Tok::Num(f) => ExprKind::Num(f),
            Tok::Str(s) => ExprKind::Str(s),
            Tok::Interp(parts) => {
                self.advance();
                return Ok(Expr { kind: ExprKind::Interp(self.interp(parts)?), pos, end: self.last_end() });
            }
            Tok::Dots => {
                if !self.vararg.last().copied().unwrap_or(false) {
                    return Err(self.error_near("cannot use '...' outside a vararg function"));
                }
                ExprKind::Dots
            }
            Tok::LBrace => return self.table(),
            Tok::Function => {
                self.advance();
                let at = self.pos();
                let body = self.func_body(false, at)?;
                return Ok(Expr { kind: ExprKind::Func(Box::new(body)), pos, end: self.last_end() });
            }
            _ => return self.suffixed(),
        };
        self.advance();
        Ok(Expr { kind, pos, end: self.last_end() })
    }

    fn interp(&mut self, parts: Vec<InterpPart>) -> PResult<Vec<InterpExpr>> {
        let mut out = Vec::new();
        for p in parts {
            match p {
                InterpPart::Lit(b) => out.push(InterpExpr::Lit(b)),
                InterpPart::Expr(text, pos) => {
                    let toks = Lexer::at(&text, self.chunk, self.dialect, pos).tokens()?;
                    let mut sub = Parser {
                        toks,
                        src: &text,
                        i: 0,
                        chunk: self.chunk,
                        dialect: self.dialect,
                        depth: self.depth,
                        vararg: self.vararg.clone(),
                        stack_base: self.stack_base,
                        stack_bytes: self.stack_bytes,
                    };
                    let e = sub.expr()?;
                    if !sub.check(&Tok::Eof) {
                        return Err(sub.error_near("'}' expected in interpolated string"));
                    }
                    out.push(InterpExpr::Expr(e));
                }
            }
        }
        Ok(out)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let pos = self.pos();
        match self.tok().clone() {
            Tok::Name(n) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Name(n), pos, end: self.last_end() })
            }
            Tok::LParen => {
                self.advance();
                let e = self.expr()?;
                self.expect_match(&Tok::RParen, ")", "(", pos)?;
                Ok(Expr { kind: ExprKind::Paren(Box::new(e)), pos, end: self.last_end() })
            }
            _ => Err(self.error_near("unexpected symbol")),
        }
    }

    fn suffixed(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        let mut chain = 0;
        loop {
            let pos = self.pos();
            chain += 1;
            if self.depth + chain / 4 > MAX_DEPTH {
                return Err(Diagnostic::error(self.chunk, pos, "expression too complex"));
            }
            match self.tok().clone() {
                Tok::Dot => {
                    self.advance();
                    let n = self.name()?;
                    let k = Expr { kind: ExprKind::Str(n.into_bytes()), pos, end: self.last_end() };
                    e = Expr { kind: ExprKind::Index(Box::new(e), Box::new(k)), pos, end: self.last_end() };
                }
                Tok::LBracket => {
                    self.advance();
                    let k = self.expr()?;
                    self.expect(&Tok::RBracket, "]")?;
                    e = Expr { kind: ExprKind::Index(Box::new(e), Box::new(k)), pos, end: self.last_end() };
                }
                Tok::Colon => {
                    self.advance();
                    let npos = self.pos();
                    let n = self.name()?;
                    let pos = self.pos();
                    let args = self.args()?;
                    e = Expr { kind: ExprKind::Method(Box::new(e), n, npos, args), pos, end: self.last_end() };
                }
                Tok::LParen | Tok::Str(_) | Tok::LBrace => {
                    let pos = self.pos();
                    let args = self.args()?;
                    e = Expr { kind: ExprKind::Call(Box::new(e), args), pos, end: self.last_end() };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn args(&mut self) -> PResult<Vec<Expr>> {
        let pos = self.pos();
        match self.tok().clone() {
            Tok::Str(s) => {
                self.advance();
                Ok(vec![Expr { kind: ExprKind::Str(s), pos, end: self.last_end() }])
            }
            Tok::LBrace => Ok(vec![self.table()?]),
            Tok::LParen => {
                self.advance();
                if self.accept(&Tok::RParen) {
                    return Ok(Vec::new());
                }
                let v = self.exprlist()?;
                self.expect_match(&Tok::RParen, ")", "(", pos)?;
                Ok(v)
            }
            _ => Err(self.error_near("function arguments expected")),
        }
    }

    fn table(&mut self) -> PResult<Expr> {
        let pos = self.pos();
        self.expect(&Tok::LBrace, "{")?;
        let mut fields = Vec::new();
        loop {
            if self.check(&Tok::RBrace) {
                break;
            }
            let fpos = self.pos();
            if let (Tok::Name(n), Tok::Assign) = (self.tok().clone(), self.tok_at(1)) {
                self.advance();
                self.advance();
                let v = self.expr()?;
                fields.push(Field::Named(n, v, fpos));
            } else if self.check(&Tok::LBracket) {
                self.advance();
                let k = self.expr()?;
                self.expect(&Tok::RBracket, "]")?;
                self.expect(&Tok::Assign, "=")?;
                let v = self.expr()?;
                fields.push(Field::Keyed(k, v));
            } else {
                fields.push(Field::Pos(self.expr()?));
            }
            if !self.accept(&Tok::Comma) && !self.accept(&Tok::Semi) {
                break;
            }
        }
        self.expect_match(&Tok::RBrace, "}", "{", pos)?;
        Ok(Expr { kind: ExprKind::Table(fields), pos, end: self.last_end() })
    }

    fn func_body(&mut self, is_method: bool, pos: Pos) -> PResult<FuncBody> {
        self.enter()?;
        let r = self.func_body_inner(is_method, pos);
        self.leave();
        r
    }

    fn func_body_inner(&mut self, is_method: bool, pos: Pos) -> PResult<FuncBody> {
        self.expect(&Tok::LParen, "(")?;
        let mut params = Vec::new();
        let mut vararg = false;
        let mut vararg_ty = None;
        if !self.check(&Tok::RParen) {
            loop {
                if self.accept(&Tok::Dots) {
                    vararg = true;
                    if self.rl() && self.accept(&Tok::Colon) {
                        // `...: T`: the varargs' type.
                        vararg_ty = Some(self.ty()?);
                    }
                    break;
                }
                let ppos = self.pos();
                let name = match self.tok().clone() {
                    Tok::Name(n) => {
                        self.advance();
                        n
                    }
                    _ => return Err(self.error_near("<name> or '...' expected")),
                };
                let ty = self.opt_annotation()?;
                params.push(Param { name, ty, pos: ppos });
                if !self.accept(&Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(&Tok::RParen, ")")?; // a plain check in C Lua (no "to close")
        let ret = if self.rl() && self.accept(&Tok::Colon) { Some(self.ret_types()?) } else { None };
        self.vararg.push(vararg);
        let body = self.block()?;
        self.vararg.pop();
        let end_line = self.pos().line;
        self.expect_match(&Tok::End, "end", "function", pos)?;
        let close_line = self.tok_end().line;
        Ok(FuncBody { params, vararg, vararg_ty, ret, body, pos, end_line, close_line, is_method })
    }

    // ---- types ----

    fn opt_annotation(&mut self) -> PResult<Option<Type>> {
        if self.rl() && self.accept(&Tok::Colon) { Ok(Some(self.ty()?)) } else { Ok(None) }
    }

    /// Return types: `T` or `(T, U)` or `()`.
    fn ret_types(&mut self) -> PResult<Vec<Type>> {
        if self.check(&Tok::LParen) {
            let save = self.i;
            self.advance();
            let mut v = Vec::new();
            if !self.accept(&Tok::RParen) {
                loop {
                    v.push(self.ty()?);
                    if !self.accept(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::RParen, ")")?;
            }
            if self.check(&Tok::Arrow) {
                // It was a function type after all.
                self.i = save;
                return Ok(vec![self.ty()?]);
            }
            return Ok(v);
        }
        Ok(vec![self.ty()?])
    }

    pub(crate) fn ty(&mut self) -> PResult<Type> {
        self.enter()?;
        let mut alts = vec![self.ty_opt()?];
        while self.accept(&Tok::Pipe) {
            alts.push(self.ty_opt()?);
        }
        self.leave();
        Ok(if alts.len() == 1 { alts.pop().unwrap_or(Type::Nil) } else { Type::Union(alts) })
    }

    fn ty_opt(&mut self) -> PResult<Type> {
        let mut t = self.ty_base()?;
        while self.accept(&Tok::Question) {
            t = Type::Optional(Box::new(t));
        }
        Ok(t)
    }

    fn ty_base(&mut self) -> PResult<Type> {
        let pos = self.pos();
        match self.tok().clone() {
            Tok::Nil => {
                self.advance();
                Ok(Type::Nil)
            }
            Tok::Function => {
                self.advance();
                Ok(Type::Named("function".into(), pos))
            }
            Tok::Name(n) => {
                self.advance();
                let mut n = n;
                while self.accept(&Tok::Dot) {
                    n.push('.');
                    n.push_str(&self.name()?);
                }
                Ok(Type::Named(n, pos))
            }
            Tok::Str(s) => {
                self.advance();
                Ok(Type::Literal(s))
            }
            Tok::LBrace => {
                self.advance();
                if self.accept(&Tok::RBrace) {
                    return Ok(Type::Shape(Vec::new()));
                }
                if self.accept(&Tok::LBracket) {
                    let k = self.ty()?;
                    self.expect(&Tok::RBracket, "]")?;
                    self.expect(&Tok::Colon, ":")?;
                    let v = self.ty()?;
                    self.accept(&Tok::Comma);
                    self.expect(&Tok::RBrace, "}")?;
                    return Ok(Type::Map(Box::new(k), Box::new(v)));
                }
                if let (Tok::Name(_), Tok::Colon) = (self.tok(), self.tok_at(1)) {
                    let mut fields = Vec::new();
                    loop {
                        if self.check(&Tok::RBrace) {
                            break;
                        }
                        let n = self.name()?;
                        self.expect(&Tok::Colon, ":")?;
                        let t = self.ty()?;
                        fields.push((n, t));
                        if !self.accept(&Tok::Comma) && !self.accept(&Tok::Semi) {
                            break;
                        }
                    }
                    self.expect(&Tok::RBrace, "}")?;
                    return Ok(Type::Shape(fields));
                }
                let elem = self.ty()?;
                self.expect(&Tok::RBrace, "}")?;
                Ok(Type::Array(Box::new(elem)))
            }
            Tok::LParen => {
                self.advance();
                let mut params = Vec::new();
                let mut vararg = false;
                if !self.check(&Tok::RParen) {
                    loop {
                        if self.accept(&Tok::Dots) {
                            vararg = true;
                            break;
                        }
                        // Parameter names are allowed and ignored: `(x: number) -> ()`.
                        if let (Tok::Name(_), Tok::Colon) = (self.tok(), self.tok_at(1)) {
                            self.advance();
                            self.advance();
                        }
                        params.push(self.ty()?);
                        if !self.accept(&Tok::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&Tok::RParen, ")")?;
                if self.accept(&Tok::Arrow) {
                    let ret = self.ret_types()?;
                    return Ok(Type::Func { params, ret, vararg });
                }
                if params.len() == 1 && !vararg {
                    return Ok(params.pop().unwrap_or(Type::Nil));
                }
                Err(self.error_near("'->' expected"))
            }
            _ => Err(self.error_near("type expected")),
        }
    }
}

/// Folds `-5`, `-5.0`, `~5` and `not` of a constant into constants, as Lua's code
/// generator does (`ast::fold_un`).
fn fold_unary(op: UnOp, e: Expr, pos: Pos) -> Expr {
    let end = e.end;
    let v = match &e.kind {
        // (`not "s"` is false: the one string fold.)
        ExprKind::Str(_) if op == UnOp::Not => Some(crate::Value::Bool(false)),
        k => crate::ast::literal_value(k).and_then(|v| crate::ast::fold_un(op, v)),
    };
    match v.and_then(crate::ast::literal_kind) {
        Some(kind) => Expr { kind, pos, end },
        None => Expr { kind: ExprKind::Un(op, Box::new(e)), pos, end },
    }
}

/// Folds arithmetic on numeric constants as Lua's `constfolding` does (`ast::fold_bin`).
fn fold_binary(op: BinOp, l: Expr, r: Expr, pos: Pos) -> Expr {
    let end = r.end;
    let num = |e: &ExprKind| matches!(e, ExprKind::Int(_) | ExprKind::Num(_)).then(|| crate::ast::literal_value(e)).flatten();
    if let (Some(a), Some(b)) = (num(&l.kind), num(&r.kind))
        && let Some(kind) = crate::ast::fold_bin(op, a, b).and_then(crate::ast::literal_kind)
    {
        return Expr { kind, pos, end };
    }
    Expr { kind: ExprKind::Bin(op, Box::new(l), Box::new(r)), pos, end }
}
