use crate::ast::*;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferType {
    Int8,
    Int16,
    Int32,
    Int64,
    Float16,
    Float32,
    Float64,
    Bool,
    Str,
    Char,
    Byte,
    Named(String),
    Unknown(String),
}

#[derive(Debug, Clone)]
struct FunctionSig {
    params: Vec<InferType>,
    ret: InferType,
}

impl InferType {
    fn from_annotation(ty: &Type) -> Self {
        match ty.name.as_str() {
            "Int8" => InferType::Int8,
            "Int16" => InferType::Int16,
            "Int32" => InferType::Int32,
            "Int64" => InferType::Int64,
            "Float16" => InferType::Float16,
            "Float32" => InferType::Float32,
            "Float64" => InferType::Float64,
            "Bool" => InferType::Bool,
            "Str" => InferType::Str,
            "Char" => InferType::Char,
            "Byte" => InferType::Byte,
            other => InferType::Named(other.to_string()),
        }
    }
}

#[derive(Debug)]
pub enum TypeError {
    UnknownIdentifier(String),
    TypeMismatch {
        expected: InferType,
        actual: InferType,
        context: String,
    },
}

pub struct TypeChecker {
    scopes: Vec<HashMap<String, InferType>>,
    func_sigs: HashMap<String, FunctionSig>,
}

impl TypeChecker {
    pub fn new() -> Self {
        TypeChecker {
            scopes: vec![HashMap::new()],
            func_sigs: HashMap::new(),
        }
    }

    pub fn check_module(&mut self, module: &Module) -> Result<(), TypeError> {
        self.collect_function_sigs(module);
        for decl in &module.declarations {
            self.check_declaration(decl)?;
        }
        Ok(())
    }

    fn collect_function_sigs(&mut self, module: &Module) {
        self.func_sigs.clear();
        for decl in &module.declarations {
            if let Declaration::Function {
                name,
                params,
                return_type,
                ..
            } = decl
            {
                let ret = return_type
                    .as_ref()
                    .map(|ty| InferType::from_annotation(ty))
                    .unwrap_or(InferType::Unknown("void".into()));
                let param_types = params
                    .iter()
                    .map(|p| InferType::from_annotation(&p.type_annotation))
                    .collect();
                self.func_sigs.insert(
                    name.name.clone(),
                    FunctionSig {
                        params: param_types,
                        ret,
                    },
                );
            }
        }
    }

    fn check_declaration(&mut self, declaration: &Declaration) -> Result<(), TypeError> {
        if let Declaration::Function {
            params,
            return_type,
            body,
            ..
        } = declaration
        {
            let expected = return_type
                .as_ref()
                .map(|ty| InferType::from_annotation(ty))
                .unwrap_or(InferType::Unknown("".into()));

            self.push_scope();
            for param in params {
                let ty = InferType::from_annotation(&param.type_annotation);
                self.declare(&param.name.name, ty);
            }
            self.check_block(body, &expected)?;
            self.pop_scope();
        }
        Ok(())
    }

    fn check_block(
        &mut self,
        block: &Block,
        expected: &InferType,
    ) -> Result<Option<InferType>, TypeError> {
        for stmt in &block.statements {
            self.check_statement(stmt, expected)?;
        }
        if let Some(expr) = &block.trailing_expression {
            let ty = self.check_expression(expr)?;
            if expected != &InferType::Unknown(String::new()) && expected != &ty {
                return Err(TypeError::TypeMismatch {
                    expected: expected.clone(),
                    actual: ty,
                    context: "block trailing expression".to_string(),
                });
            }
            Ok(Some(ty))
        } else {
            Ok(None)
        }
    }

    fn check_statement(&mut self, stmt: &Statement, expected: &InferType) -> Result<(), TypeError> {
        match stmt {
            Statement::LetBinding {
                name,
                type_annotation,
                initializer,
                ..
            } => {
                let init_ty = self.check_expression(initializer)?;
                let declared_ty = type_annotation
                    .as_ref()
                    .map(|ty| InferType::from_annotation(ty))
                    .unwrap_or(init_ty.clone());
                if init_ty != declared_ty {
                    return Err(TypeError::TypeMismatch {
                        expected: declared_ty,
                        actual: init_ty,
                        context: name.name.clone(),
                    });
                }
                self.declare(&name.name, declared_ty);
                Ok(())
            }
            Statement::Return(Some(expr)) => {
                let ty = self.check_expression(expr)?;
                if expected != &InferType::Unknown(String::new()) && expected != &ty {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.clone(),
                        actual: ty,
                        context: "return".to_string(),
                    });
                }
                Ok(())
            }
            Statement::Return(None) => Ok(()),
            Statement::Expression(expr) => {
                self.check_expression(expr)?;
                Ok(())
            }
            Statement::Conc { body } => {
                self.push_scope();
                self.check_block(body, &InferType::Unknown(String::new()))?;
                self.pop_scope();
                Ok(())
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let _ = self.check_expression(condition)?;
                self.push_scope();
                self.check_block(then_branch, &InferType::Unknown(String::new()))?;
                self.pop_scope();
                if let Some(ElseBranch::Block(block)) = else_branch {
                    self.push_scope();
                    self.check_block(block, expected)?;
                    self.pop_scope();
                }
                Ok(())
            }
            Statement::Loop { kind, body } => {
                match kind {
                    LoopKind::For { pattern, iterator, body: _ } => {
                        // Check iterator first and try to infer element type for pattern bindings
                        let _ = self.check_expression(iterator)?;
                        let elem_ty = self.infer_element_type_of_iterator(iterator);
                        self.push_scope();
                        self.declare_pattern_in_scope_with_type(pattern, elem_ty);
                        self.check_block(body, &InferType::Unknown(String::new()))?;
                        self.pop_scope();
                        Ok(())
                    }
                    LoopKind::While { condition, body: _ } => {
                        let _ = self.check_expression(condition)?;
                        self.push_scope();
                        self.check_block(body, &InferType::Unknown(String::new()))?;
                        self.pop_scope();
                        Ok(())
                    }
                    LoopKind::Block(block) => {
                        self.push_scope();
                        self.check_block(block, &InferType::Unknown(String::new()))?;
                        self.pop_scope();
                        Ok(())
                    }
                }
            }
            _ => Ok(()),
        }
    }

    fn check_expression(&mut self, expr: &Expression) -> Result<InferType, TypeError> {
        match expr {
            Expression::Literal(lit) => Ok(self.type_of_literal(lit)),
            Expression::Identifier(id) => self
                .lookup(&id.name)
                .cloned()
                .ok_or(TypeError::UnknownIdentifier(id.name.clone())),
            Expression::Block(block) => {
                self.push_scope();
                let block_ty = self.check_block(block, &InferType::Unknown(String::new()));
                self.pop_scope();
                if let Some(ty) = block_ty? {
                    Ok(ty)
                } else {
                    Ok(InferType::Unknown("block".into()))
                }
            }
            Expression::MergeExpression { base, fields } => {
                if let Some(base_expr) = base {
                    self.check_expression(base_expr)?;
                }
                for (_, expr) in fields {
                    self.check_expression(expr)?;
                }
                Ok(InferType::Unknown("merge".into()))
            }
            Expression::BinaryOp { op, left, right } => {
                let lhs = self.check_expression(left)?;
                let rhs = self.check_expression(right)?;
                match op {
                    Operator::Add
                    | Operator::Sub
                    | Operator::Mul
                    | Operator::Div
                    | Operator::Mod => {
                        if lhs == InferType::Int32 && rhs == InferType::Int32 {
                            Ok(InferType::Int32)
                        } else {
                            Err(TypeError::TypeMismatch {
                                expected: InferType::Int32,
                                actual: rhs,
                                context: format!("arithmetic binary {:?}", op),
                            })
                        }
                    }
                    Operator::AddAssign | Operator::SubAssign | Operator::MulAssign | Operator::DivAssign => {
                        // compound-assign: lhs must be assignable and types must match
                        // We only check type compatibility here
                        if lhs == rhs {
                            Ok(lhs)
                        } else {
                            Err(TypeError::TypeMismatch {
                                expected: lhs,
                                actual: rhs,
                                context: format!("compound assign {:?}", op),
                            })
                        }
                    }
                    Operator::Pipe => {
                        // pipe: left |> right  — if right is a call to a known function, ensure types align
                        // we can't fully model generics here; accept when types match param[0] if available
                        if let Expression::Call { func, args } = right.as_ref() {
                            if let Expression::Identifier(id) = func.as_ref() {
                                if let Some(sig) = self.func_sigs.get(&id.name) {
                                    if !sig.params.is_empty() {
                                        let first = sig.params[0].clone();
                                        let expected = first.clone();
                                        if lhs == expected {
                                            return Ok(sig.ret.clone());
                                        }
                                    }
                                }
                            }
                        }
                        Ok(InferType::Unknown("pipe".into()))
                    }
                    Operator::Eq
                    | Operator::Ne
                    | Operator::Lt
                    | Operator::Le
                    | Operator::Gt
                    | Operator::Ge => {
                        if lhs == rhs {
                            Ok(InferType::Bool)
                        } else {
                            Err(TypeError::TypeMismatch {
                                expected: lhs,
                                actual: rhs,
                                context: format!("comparison {:?}", op),
                            })
                        }
                    }
                    Operator::And | Operator::Or => {
                        if lhs == InferType::Bool && rhs == InferType::Bool {
                            Ok(InferType::Bool)
                        } else {
                            Err(TypeError::TypeMismatch {
                                expected: InferType::Bool,
                                actual: rhs,
                                context: format!("logical {:?}", op),
                            })
                        }
                    }
                    Operator::BitAnd
                    | Operator::BitOr
                    | Operator::BitXor
                    | Operator::Shl
                    | Operator::Shr => {
                        if lhs == InferType::Int32 && rhs == InferType::Int32 {
                            Ok(InferType::Int32)
                        } else {
                            Err(TypeError::TypeMismatch {
                                expected: InferType::Int32,
                                actual: rhs,
                                context: format!("bitwise {:?}", op),
                            })
                        }
                    }
                    _ => Ok(InferType::Unknown("binary".into())),
                }
            }
            Expression::Call { func, args } => {
                let func_name = if let Expression::Identifier(id) = func.as_ref() {
                    id.name.clone()
                } else {
                    return Err(TypeError::UnknownIdentifier("call".into()));
                };
                let sig = self
                    .func_sigs
                    .get(&func_name)
                    .cloned()
                    .ok_or(TypeError::UnknownIdentifier(func_name.clone()))?;
                if sig.params.len() != args.len() {
                    return Err(TypeError::TypeMismatch {
                        expected: InferType::Unknown("arity".into()),
                        actual: InferType::Unknown("args".into()),
                        context: func_name.clone(),
                    });
                }
                for (idx, arg) in args.iter().enumerate() {
                    let ty = self.check_expression(arg)?;
                    if ty != sig.params[idx] {
                        return Err(TypeError::TypeMismatch {
                            expected: sig.params[idx].clone(),
                            actual: ty,
                            context: func_name.clone(),
                        });
                    }
                }
                Ok(sig.ret.clone())
            }
            Expression::TryOperator { expr } => {
                // try operator: propagate inner expression's type for now
                self.check_expression(expr)
            }
            _ => Ok(InferType::Unknown("expr".into())),
        }
    }

    fn type_of_literal(&self, lit: &Literal) -> InferType {
        match lit {
            Literal::Int(_val, suffix) => {
                if let Some(s) = suffix {
                    if s.starts_with('i') {
                        match s.as_str() {
                            "i8" => InferType::Int8,
                            "i16" => InferType::Int16,
                            "i32" => InferType::Int32,
                            "i64" => InferType::Int64,
                            _ => InferType::Int32,
                        }
                    } else if s.starts_with('u') {
                        match s.as_str() {
                            "u8" => InferType::Byte,
                            _ => InferType::Named(s.clone()),
                        }
                    } else {
                        InferType::Int32
                    }
                } else {
                    InferType::Int32
                }
            }
            Literal::Float(_val, suffix) => {
                if let Some(s) = suffix {
                    match s.as_str() {
                        "f32" => InferType::Float32,
                        "f64" => InferType::Float64,
                        _ => InferType::Float32,
                    }
                } else {
                    InferType::Float32
                }
            }
            Literal::Bool(_) => InferType::Bool,
            Literal::Str(_) => InferType::Str,
            Literal::Array(elems) => {
                if elems.is_empty() {
                    return InferType::Unknown("array".into());
                }
                // infer element type if all literals match
                let first_ty = match &elems[0] {
                    Expression::Literal(l) => self.type_of_literal(l),
                    _ => return InferType::Unknown("array".into()),
                };
                for e in elems.iter().skip(1) {
                    match e {
                        Expression::Literal(l) => {
                            if self.type_of_literal(l) != first_ty {
                                return InferType::Unknown("array".into());
                            }
                        }
                        _ => return InferType::Unknown("array".into()),
                    }
                }
                InferType::Named(format!("Array<{:?}>", first_ty))
            }
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &str, ty: InferType) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), ty);
        }
    }

    fn lookup(&self, name: &str) -> Option<&InferType> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty);
            }
        }
        None
    }

    fn declare_pattern_in_scope(&mut self, pattern: &Pattern) {
        self.declare_pattern_in_scope_with_type(pattern, None);
    }

    fn declare_pattern_in_scope_with_type(&mut self, pattern: &Pattern, ty: Option<InferType>) {
        match pattern {
            Pattern::Wildcard => {}
            Pattern::Identifier(id) => {
                let bind_ty = ty.clone().unwrap_or(InferType::Unknown("for-pat".into()));
                self.declare(&id.name, bind_ty);
            }
            Pattern::Tuple(elems) | Pattern::Array(elems) => {
                for p in elems {
                    self.declare_pattern_in_scope_with_type(p, ty.clone());
                }
            }
            Pattern::Struct { fields, .. } => {
                for (_id, p) in fields {
                    self.declare_pattern_in_scope_with_type(p, ty.clone());
                }
            }
            Pattern::Guard { pattern: p, .. } => self.declare_pattern_in_scope_with_type(p, ty.clone()),
            Pattern::EnumVariant { payload: Some(p), .. } => self.declare_pattern_in_scope_with_type(p, ty.clone()),
            _ => {}
        }
    }

    fn infer_element_type_of_iterator(&self, iterator: &Expression) -> Option<InferType> {
        match iterator {
            Expression::Literal(Literal::Array(elems)) => {
                if elems.is_empty() {
                    return None;
                }
                // Infer type from first element and ensure all match
                let first_ty = match &elems[0] {
                    Expression::Literal(l) => self.type_of_literal(l),
                    _ => return None,
                };
                for e in elems.iter().skip(1) {
                    match e {
                        Expression::Literal(l) => {
                            if self.type_of_literal(l) != first_ty {
                                return None;
                            }
                        }
                        _ => return None,
                    }
                }
                Some(first_ty)
            }
            Expression::Identifier(id) => {
                // Try to infer element type from a declared array variable with Named(Array<...>)
                if let Some(ty) = self.lookup(&id.name) {
                    if let InferType::Named(name) = ty {
                        if name.starts_with("Array<") && name.ends_with('>') {
                            let inner = &name[6..name.len()-1];
                            return Some(match inner {
                                "Int32" => InferType::Int32,
                                "Float32" => InferType::Float32,
                                "Bool" => InferType::Bool,
                                "Str" => InferType::Str,
                                other => InferType::Named(other.to_string()),
                            });
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::resolver::Resolver;

    fn normalize_source(source: &str) -> String {
        let mut body = source.trim().to_string();
        if !body.starts_with("namespace main") {
            body = format!("namespace main\n{}", body);
        }
        if !body.contains("fn main") {
            body.push_str("\nfn main() -> Int32 { return 0; }");
        }
        body
    }

    fn check(source: &str) -> Result<(), TypeError> {
        let module = Parser::new(Lexer::new(normalize_source(source)).tokenize())
            .parse_module()
            .unwrap();
        let mut resolver = Resolver::new();
        resolver.resolve_module(&module).unwrap();
        let mut checker = TypeChecker::new();
        checker.check_module(&module)
    }

    #[test]
    fn accepts_simple_function() {
        let source =
            "fn compute(count: Int32) -> Int32 { let accumulator: Int32 = 0; return accumulator; }";
        assert!(check(source).is_ok());
    }

    #[test]
    fn rejects_mismatched_let_type() {
        let source = "fn bad() -> Int32 { let text: Int32 = \"hello\"; return text; }";
        let err = check(source).unwrap_err();
        match err {
            TypeError::TypeMismatch { context, .. } => assert_eq!(context, "text"),
            _ => panic!("expected type mismatch error"),
        }
    }

    #[test]
    fn accepts_named_types_option_result_buf() {
        let source =
            "fn api() -> Result<Buf, Str> { let value: Option<Buf> = \"\"; return value; }";
        let err = check(source).unwrap_err();
        match err {
            TypeError::TypeMismatch { expected, .. } => match expected {
                InferType::Named(name) => assert_eq!(name, "Option"),
                _ => panic!("expected named type for Option"),
            },
            _ => panic!("expected type mismatch error"),
        }
    }

    #[test]
    fn literal_suffix_types() {
        assert!(check("fn i8f() -> Int8 { return 42i8; }").is_ok());
        assert!(check("fn i16f() -> Int16 { return 100i16; }").is_ok());
        assert!(check("fn i64f() -> Int64 { return 900i64; }").is_ok());
        assert!(check("fn float64f() -> Float64 { return 3.14f64; }").is_ok());
        assert!(check("fn bytef() -> Byte { return 255u8; }").is_ok());
    }

    #[test]
    fn arithmetic_int32_accepts() {
        assert!(check("fn add() -> Int32 { return 1 + 2; }").is_ok());
    }

    #[test]
    fn arithmetic_i8_rejects() {
        let err = check("fn addi8() -> Int8 { return 1i8 + 2i8; }").unwrap_err();
        match err {
            TypeError::TypeMismatch { .. } => (),
            _ => panic!("expected type mismatch for i8 arithmetic"),
        }
    }

    #[test]
    fn bitwise_shift_accepts() {
        assert!(check("fn shl() -> Int32 { return 1 << 2; }").is_ok());
    }
}
