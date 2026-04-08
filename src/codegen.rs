use crate::ast::*;
use crate::span::Span;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct IrModule {
    pub functions: Vec<IrFunction>,
    pub preamble: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrFunction {
    pub name: String,
    pub body: String,
    pub ret_type: String,
    pub params: Vec<(String, String)>,
}

pub struct Codegen;

impl Codegen {
    pub fn lower_module(module: &Module) -> IrModule {
        let mut functions = Vec::new();
        let mut builder = IrBuilder::new();
        let preamble = builder.collect_types(module);
        for decl in &module.declarations {
            if let DeclarationKind::Function {
                name,
                return_type,
                body,
                params,
                ..
            } = &decl.node
            {
                let ret_ty = return_type
                    .as_ref()
                    .map(|ty| builder.lower_type(ty))
                    .unwrap_or_else(|| "void".to_string());
                let body_ir = builder.emit_function(name, params, &ret_ty, body);
                let param_list = params
                    .iter()
                    .map(|p| (p.name.name.clone(), builder.lower_type(&p.type_annotation)))
                    .collect();
                functions.push(IrFunction {
                    name: name.name.clone(),
                    body: body_ir,
                    ret_type: ret_ty,
                    params: param_list,
                });
            }
        }
        IrModule {
            functions,
            preamble,
        }
    }
}

struct IrBuilder {
    lines: Vec<String>,
    next_tmp: usize,
    last_value: Option<String>,
    locals: HashMap<String, String>,
    locals_type: HashMap<String, String>,
    type_decls: Vec<String>,
    next_label: usize,
    struct_fields: HashMap<String, Vec<(String, String)>>,
    func_sigs: HashMap<String, (String, Vec<String>)>,
}

impl IrBuilder {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            next_tmp: 0,
            last_value: None,
            locals: HashMap::new(),
            locals_type: HashMap::new(),
            type_decls: Vec::new(),
            next_label: 0,
            struct_fields: HashMap::new(),
            func_sigs: HashMap::new(),
        }
    }

    fn collect_types(&mut self, module: &Module) -> Vec<String> {
        self.type_decls.clear();
        self.struct_fields.clear();
        self.func_sigs.clear();
        for decl in &module.declarations {
            match &decl.node {
                DeclarationKind::Struct { name, fields, .. } => {
                    let mut field_types = Vec::new();
                    let mut field_map = Vec::new();
                    for (field, ty) in fields {
                        let lowered = self.lower_type(ty);
                        field_types.push(lowered.clone());
                        field_map.push((field.name.clone(), lowered));
                    }
                    let body = field_types.join(", ");
                    self.type_decls
                        .push(format!("%struct.{} = type {{ {} }}", name.name, body));
                    self.struct_fields.insert(name.name.clone(), field_map);
                }
                DeclarationKind::Enum { name, .. } => {
                    self.type_decls
                        .push(format!("%enum.{} = type opaque", name.name));
                }
                DeclarationKind::Newtype { name, type_alias } => {
                    let alias = self.lower_type(type_alias);
                    self.type_decls
                        .push(format!("%newtype.{} = type {}", name.name, alias));
                }
                DeclarationKind::Function {
                    name,
                    return_type,
                    params,
                    ..
                } => {
                    let ret_ty = return_type
                        .as_ref()
                        .map(|ty| self.lower_type(ty))
                        .unwrap_or_else(|| "void".to_string());
                    let param_types = params
                        .iter()
                        .map(|p| self.lower_type(&p.type_annotation))
                        .collect();
                    self.func_sigs
                        .insert(name.name.clone(), (ret_ty, param_types));
                }
                _ => {}
            }
        }
        self.type_decls.clone()
    }

    fn emit_function(
        &mut self,
        _name: &Identifier,
        params: &[Parameter],
        ret_ty: &str,
        body: &Block,
    ) -> String {
        self.lines.clear();
        self.locals.clear();
        self.last_value = None;
        self.lines.push("entry:".to_string());

        for param in params {
            let param_ty = self.lower_type(&param.type_annotation);
            let slot = self.next_register();
            self.lines.push(format!("  {} = alloca {}", slot, param_ty));
            self.lines.push(format!(
                "  store {} %{}, {}* {}",
                param_ty, param.name.name, param_ty, slot
            ));
            self.locals.insert(param.name.name.clone(), slot);
            self.locals_type.insert(param.name.name.clone(), param_ty);
        }

        self.emit_block(body, ret_ty);
        self.finish(ret_ty)
    }

    fn finish(&mut self, return_type: &str) -> String {
        let has_ret = self
            .lines
            .iter()
            .any(|l| l.trim_start().starts_with("ret ") || l.trim_start().starts_with("ret\t"));

        if let Some(value) = self.last_value.take() {
            if return_type != "void" {
                if !has_ret {
                    self.lines.push(format!("  ret {} {}", return_type, value));
                }
            } else if !has_ret {
                self.lines.push("  ret void".to_string());
            }
        } else if !has_ret {
            self.lines.push("  ret void".to_string());
        }

        if let Some(last) = self.lines.last() {
            if last.trim_end().ends_with(':') {
                if return_type != "void" {
                    self.lines.push(format!("  ret {} 0", return_type));
                } else {
                    self.lines.push("  ret void".to_string());
                }
            }
        }

        self.lines.join("\n")
    }

    fn emit_block(&mut self, block: &Block, current_fn_ret_ty: &str) {
        self.annotate_span(&block.span);
        for stmt in &block.statements {
            match &stmt.node {
                StatementKind::Return(Some(expr)) => {
                    let value = self.emit_expr(expr);
                    self.last_value = Some(value);
                    return;
                }
                StatementKind::Return(None) => {
                    self.last_value = None;
                    return;
                }
                StatementKind::LetBinding {
                    name,
                    initializer,
                    type_annotation,
                    ..
                } => {
                    if let ExpressionKind::Literal(Literal {
                        kind: LiteralKind::Array(elems),
                        ..
                    }) = &initializer.node
                    {
                        let len = elems.len();
                        let elem_ty = if let Some(first) = elems.get(0) {
                            match &first.node {
                                ExpressionKind::Literal(Literal {
                                    kind: LiteralKind::Int(_, suffix),
                                    ..
                                }) => {
                                    if let Some(s) = suffix {
                                        match s.as_str() {
                                            "i8" => "i8".to_string(),
                                            "i16" => "i16".to_string(),
                                            "i32" => "i32".to_string(),
                                            "i64" => "i64".to_string(),
                                            "u8" => "i8".to_string(),
                                            _ => "i32".to_string(),
                                        }
                                    } else {
                                        "i32".to_string()
                                    }
                                }
                                ExpressionKind::Literal(Literal {
                                    kind: LiteralKind::Float(_, suffix),
                                    ..
                                }) => {
                                    if let Some(s) = suffix {
                                        match s.as_str() {
                                            "f32" => "float".to_string(),
                                            "f64" => "double".to_string(),
                                            _ => "float".to_string(),
                                        }
                                    } else {
                                        "float".to_string()
                                    }
                                }
                                ExpressionKind::Literal(Literal {
                                    kind: LiteralKind::Bool(_),
                                    ..
                                }) => "i1".to_string(),
                                _ => "i32".to_string(),
                            }
                        } else {
                            "i32".to_string()
                        };
                        let array_ty = format!("[{} x {}]", len, elem_ty);
                        let alloca = self.next_register();
                        self.lines
                            .push(format!("  {} = alloca {}", alloca, array_ty));
                        for (i, elem_expr) in elems.iter().enumerate() {
                            let val = match &elem_expr.node {
                                ExpressionKind::Literal(Literal {
                                    kind: LiteralKind::Int(v, _),
                                    ..
                                }) => v.to_string(),
                                ExpressionKind::Literal(Literal {
                                    kind: LiteralKind::Float(f, _),
                                    ..
                                }) => f.to_string(),
                                ExpressionKind::Literal(Literal {
                                    kind: LiteralKind::Bool(b),
                                    ..
                                }) => {
                                    if *b {
                                        "1".to_string()
                                    } else {
                                        "0".to_string()
                                    }
                                }
                                _ => self.emit_expr(elem_expr),
                            };
                            let gep = self.next_register();
                            self.lines.push(format!(
                                "  {} = getelementptr inbounds {}, {}* {}, i32 0, i32 {}",
                                gep, array_ty, array_ty, alloca, i
                            ));
                            self.lines
                                .push(format!("  store {} {}, {}* {}", elem_ty, val, elem_ty, gep));
                        }
                        self.locals.insert(name.name.clone(), alloca.clone());
                        self.locals_type.insert(name.name.clone(), array_ty);
                    } else {
                        let value = self.emit_expr(initializer);
                        let ty = type_annotation
                            .as_ref()
                            .map(|ty| self.lower_type(ty))
                            .unwrap_or_else(|| "i32".to_string());
                        let alloca = self.next_register();
                        self.lines.push(format!("  {} = alloca {}", alloca, ty));
                        self.lines
                            .push(format!("  store {} {}, {}* {}", ty, value, ty, alloca));
                        self.locals.insert(name.name.clone(), alloca);
                        self.locals_type.insert(name.name.clone(), ty);
                    }
                }
                StatementKind::Expression(expr) => {
                    let _ = self.emit_expr(expr);
                }
                StatementKind::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    let cond_val = self.emit_expr(condition);
                    let then_label = self.next_block("then");
                    let else_label = self.next_block("else");
                    let merge_label = self.next_block("if_merge");

                    self.lines.push(format!(
                        "  br i1 {}, label %{}, label %{}",
                        cond_val, then_label, else_label
                    ));

                    self.lines.push(format!("{}:", then_label));
                    let then_terminated =
                        self.emit_block_terminated(then_branch, current_fn_ret_ty);
                    if !then_terminated {
                        self.lines.push(format!("  br label %{}", merge_label));
                    }

                    self.lines.push(format!("{}:", else_label));
                    let else_terminated = match else_branch {
                        None => {
                            self.lines.push(format!("  br label %{}", merge_label));
                            true
                        }
                        Some(eb) => match &eb.node {
                            ElseBranchKind::Block(block) => {
                                let t = self.emit_block_terminated(block, current_fn_ret_ty);
                                if !t {
                                    self.lines.push(format!("  br label %{}", merge_label));
                                }
                                t
                            }
                            ElseBranchKind::If(stmt) => {
                                self.emit_statement_terminated(stmt, current_fn_ret_ty)
                            }
                        },
                    };

                    self.lines.push(format!("{}:", merge_label));
                    if then_terminated && else_terminated {
                        self.lines.push("  unreachable".to_string());
                    }
                }
                StatementKind::Match { expr, arms } => {
                    let discr = self.emit_expr(expr);
                    let merge_label = self.next_block("match_merge");
                    for (idx, arm) in arms.iter().enumerate() {
                        let arm_label = self.next_block(&format!("match_arm_{}", idx));
                        self.lines.push(format!("  br label %{}", arm_label));
                        self.lines.push(format!("{}:", arm_label));
                        let _ = self.emit_expr(&arm.node.body);
                        self.lines.push(format!("  br label %{}", merge_label));
                    }
                    self.lines.push(format!("{}:", merge_label));
                    let _ = discr;
                }
                StatementKind::Loop { kind, body } => {
                    let loop_label = self.next_block("loop");
                    let loop_body = self.next_block("loop_body");
                    let loop_end = self.next_block("loop_end");
                    match &kind.node {
                        LoopKindKind::While { condition, .. } => {
                            self.lines.push(format!("  br label %{}", loop_label));
                            self.lines.push(format!("{}:", loop_label));
                            let cond_val = self.emit_expr(condition);
                            self.lines.push(format!(
                                "  br i1 {}, label %{}, label %{}",
                                cond_val, loop_body, loop_end
                            ));
                            self.lines.push(format!("{}:", loop_body));
                            self.emit_block(body, current_fn_ret_ty);
                            self.lines.push(format!("  br label %{}", loop_label));
                            self.lines.push(format!("{}:", loop_end));
                        }
                        LoopKindKind::For {
                            pattern, iterator, ..
                        } => {
                            if let ExpressionKind::Identifier(iter_id) = &iterator.node {
                                if let Some(iter_ty) = self.locals_type.get(&iter_id.name).cloned()
                                {
                                    if iter_ty.starts_with('[') && iter_ty.contains(" x i32") {
                                        if let Some(end_bracket) = iter_ty.find(']') {
                                            let inside = &iter_ty[1..end_bracket];
                                            if let Some(space_idx) = inside.find(' ') {
                                                let len_str = &inside[..space_idx];
                                                if let Ok(len) = len_str.parse::<usize>() {
                                                    if let PatternKind::Identifier(ident) =
                                                        &pattern.node
                                                    {
                                                        let pat_alloc = self.next_register();
                                                        self.lines.push(format!(
                                                            "  {} = alloca i32",
                                                            pat_alloc
                                                        ));
                                                        self.locals.insert(
                                                            ident.name.clone(),
                                                            pat_alloc.clone(),
                                                        );
                                                        self.locals_type.insert(
                                                            ident.name.clone(),
                                                            "i32".to_string(),
                                                        );
                                                        let idx_slot = self.next_register();
                                                        self.lines.push(format!(
                                                            "  {} = alloca i32",
                                                            idx_slot
                                                        ));
                                                        self.lines.push(format!(
                                                            "  store i32 0, i32* {}",
                                                            idx_slot
                                                        ));
                                                        let iter_alloc = self
                                                            .locals
                                                            .get(&iter_id.name)
                                                            .cloned()
                                                            .unwrap_or(iter_id.name.clone());
                                                        self.lines.push(format!(
                                                            "  br label %{}",
                                                            loop_label
                                                        ));
                                                        self.lines.push(format!("{}:", loop_label));
                                                        let idx_val = self.next_register();
                                                        self.lines.push(format!(
                                                            "  {} = load i32, i32* {}",
                                                            idx_val, idx_slot
                                                        ));
                                                        let cmp = self.next_register();
                                                        self.lines.push(format!(
                                                            "  {} = icmp slt i32 {}, {}",
                                                            cmp, idx_val, len
                                                        ));
                                                        self.lines.push(format!(
                                                            "  br i1 {}, label %{}, label %{}",
                                                            cmp, loop_body, loop_end
                                                        ));
                                                        self.lines.push(format!("{}:", loop_body));
                                                        let idx_val2 = self.next_register();
                                                        self.lines.push(format!(
                                                            "  {} = load i32, i32* {}",
                                                            idx_val2, idx_slot
                                                        ));
                                                        let gep = self.next_register();
                                                        self.lines.push(format!("  {} = getelementptr inbounds {}, {}* {}, i32 0, i32 {}", gep, iter_ty, iter_ty, iter_alloc, idx_val2));
                                                        let elem = self.next_register();
                                                        self.lines.push(format!(
                                                            "  {} = load i32, i32* {}",
                                                            elem, gep
                                                        ));
                                                        self.lines.push(format!(
                                                            "  store i32 {}, i32* {}",
                                                            elem, pat_alloc
                                                        ));
                                                        let body_terminated = self
                                                            .emit_block_terminated(
                                                                body,
                                                                current_fn_ret_ty,
                                                            );
                                                        if !body_terminated {
                                                            let next_idx = self.next_register();
                                                            self.lines.push(format!(
                                                                "  {} = add i32 {}, 1",
                                                                next_idx, idx_val2
                                                            ));
                                                            self.lines.push(format!(
                                                                "  store i32 {}, i32* {}",
                                                                next_idx, idx_slot
                                                            ));
                                                            self.lines.push(format!(
                                                                "  br label %{}",
                                                                loop_label
                                                            ));
                                                        }
                                                        self.lines.push(format!("{}:", loop_end));
                                                        continue;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            self.lines.push(format!("  br label %{}", loop_label));
                            self.lines.push(format!("{}:", loop_body));
                            self.emit_block(body, current_fn_ret_ty);
                            self.lines.push(format!("  br label %{}", loop_label));
                            self.lines.push(format!("{}:", loop_end));
                        }
                        _ => {
                            self.lines.push(format!("  br label %{}", loop_label));
                            self.lines.push(format!("{}:", loop_body));
                            self.emit_block(body, current_fn_ret_ty);
                            self.lines.push(format!("  br label %{}", loop_label));
                            self.lines.push(format!("{}:", loop_end));
                        }
                    }
                }
                StatementKind::Conc { body } => {
                    self.emit_block(body, current_fn_ret_ty);
                }
                _ => {}
            }
        }
        if let Some(expr) = &block.trailing_expression {
            let value = self.emit_expr(expr);
            self.last_value = Some(value);
        }
    }

    fn emit_block_terminated(&mut self, block: &Block, ret_ty: &str) -> bool {
        for stmt in &block.statements {
            let terminated = self.emit_statement_terminated(stmt, ret_ty);
            if terminated {
                return true;
            }
        }
        false
    }

    fn emit_statement_terminated(&mut self, stmt: &Statement, ret_ty: &str) -> bool {
        match &stmt.node {
            StatementKind::Return(Some(expr)) => {
                let value = self.emit_expr(expr);
                self.lines.push(format!("  ret {} {}", ret_ty, value));
                true
            }
            StatementKind::Return(None) => {
                self.lines.push("  ret void".to_string());
                true
            }
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let cond_val = self.emit_expr(condition);
                let then_label = self.next_block("then");
                let else_label = self.next_block("else");
                let merge_label = self.next_block("if_merge");

                self.lines.push(format!(
                    "  br i1 {}, label %{}, label %{}",
                    cond_val, then_label, else_label
                ));

                self.lines.push(format!("{}:", then_label));
                let then_terminated = self.emit_block_terminated(then_branch, ret_ty);
                if !then_terminated {
                    self.lines.push(format!("  br label %{}", merge_label));
                }

                self.lines.push(format!("{}:", else_label));
                let else_terminated = match else_branch {
                    None => {
                        self.lines.push(format!("  br label %{}", merge_label));
                        true
                    }
                    Some(eb) => match &eb.node {
                        ElseBranchKind::Block(block) => {
                            let t = self.emit_block_terminated(block, ret_ty);
                            if !t {
                                self.lines.push(format!("  br label %{}", merge_label));
                            }
                            t
                        }
                        ElseBranchKind::If(stmt) => self.emit_statement_terminated(stmt, ret_ty),
                    },
                };

                self.lines.push(format!("{}:", merge_label));
                if then_terminated && else_terminated {
                    self.lines.push("  unreachable".to_string());
                    return true;
                }
                false
            }
            StatementKind::Match { expr, arms } => {
                let merge_label = self.next_block("match_merge");
                for (idx, arm) in arms.iter().enumerate() {
                    let arm_label = self.next_block(&format!("match_arm_{}", idx));
                    self.lines.push(format!("  br label %{}", arm_label));
                    self.lines.push(format!("{}:", arm_label));
                    let _ = self.emit_expr(&arm.node.body);
                    self.lines.push(format!("  br label %{}", merge_label));
                }
                self.lines.push(format!("{}:", merge_label));
                false
            }
            StatementKind::Loop { kind, body } => {
                match &kind.node {
                    LoopKindKind::While { condition, .. } => {
                        let loop_label = self.next_block("loop");
                        let loop_body = self.next_block("loop_body");
                        let loop_end = self.next_block("loop_end");
                        self.lines.push(format!("  br label %{}", loop_label));
                        self.lines.push(format!("{}:", loop_label));
                        let cond_val = self.emit_expr(condition);
                        self.lines.push(format!(
                            "  br i1 {}, label %{}, label %{}",
                            cond_val, loop_body, loop_end
                        ));
                        self.lines.push(format!("{}:", loop_body));
                        self.emit_block(body, ret_ty);
                        self.lines.push(format!("  br label %{}", loop_label));
                        self.lines.push(format!("{}:", loop_end));
                    }
                    _ => {
                        self.emit_statement(stmt, ret_ty);
                    }
                }
                false
            }
            other => {
                self.emit_statement(stmt, ret_ty);
                false
            }
        }
    }

    fn emit_statement(&mut self, stmt: &Statement, current_fn_ret_ty: &str) {
        match &stmt.node {
            StatementKind::Return(Some(expr)) => {
                let value = self.emit_expr(expr);
                self.last_value = Some(value);
            }
            StatementKind::Return(None) => {
                self.last_value = None;
            }
            StatementKind::Expression(expr) => {
                let _ = self.emit_expr(expr);
            }
            StatementKind::If { .. } | StatementKind::Match { .. } | StatementKind::Loop { .. } => {
                self.emit_block(
                    &Block {
                        statements: vec![stmt.clone()],
                        trailing_expression: None,
                        span: stmt.span,
                    },
                    current_fn_ret_ty,
                );
            }
            _ => {}
        }
    }

    fn annotate_span(&mut self, span: &Span) {
        if *span == Span::default() {
            return;
        }
        self.lines.push(format!(
            "  ; span {}..{} @ {}:{}",
            span.start, span.end, span.line, span.col
        ));
    }

    fn emit_expr(&mut self, expr: &Expression) -> String {
        match &expr.node {
            ExpressionKind::Literal(Literal {
                kind: LiteralKind::Int(value, _),
                ..
            }) => value.to_string(),
            ExpressionKind::Literal(Literal {
                kind: LiteralKind::Bool(value),
                ..
            }) => {
                if *value {
                    "1".to_string()
                } else {
                    "0".to_string()
                }
            }
            ExpressionKind::Identifier(id) => {
                if let Some(slot) = self.locals.get(&id.name).cloned() {
                    let ty = self
                        .locals_type
                        .get(&id.name)
                        .cloned()
                        .unwrap_or_else(|| "i32".to_string());
                    let tmp = self.next_register();
                    self.lines
                        .push(format!("  {} = load {}, {}* {}", tmp, ty, ty, slot));
                    tmp
                } else {
                    id.name.clone()
                }
            }
            ExpressionKind::BinaryOp { op, left, right } => {
                fn is_float_ty(s: &str) -> bool {
                    matches!(s, "float" | "double" | "half")
                }
                fn is_bool_ty(s: &str) -> bool {
                    s == "i1"
                }

                let ty = match &left.node {
                    ExpressionKind::Identifier(id) => self
                        .locals_type
                        .get(&id.name)
                        .cloned()
                        .unwrap_or_else(|| "i32".to_string()),
                    ExpressionKind::Literal(Literal {
                        kind: LiteralKind::Int(_, suffix),
                        ..
                    }) => {
                        if let Some(s) = suffix {
                            match s.as_str() {
                                "i8" => "i8".to_string(),
                                "i16" => "i16".to_string(),
                                "i32" => "i32".to_string(),
                                "i64" => "i64".to_string(),
                                "u8" => "i8".to_string(),
                                _ => "i32".to_string(),
                            }
                        } else {
                            "i32".to_string()
                        }
                    }
                    ExpressionKind::Literal(Literal {
                        kind: LiteralKind::Float(_, suffix),
                        ..
                    }) => {
                        if let Some(s) = suffix {
                            match s.as_str() {
                                "f32" => "float".to_string(),
                                "f64" => "double".to_string(),
                                _ => "float".to_string(),
                            }
                        } else {
                            "float".to_string()
                        }
                    }
                    ExpressionKind::Literal(Literal {
                        kind: LiteralKind::Bool(_),
                        ..
                    }) => "i1".to_string(),
                    ExpressionKind::Call { func, .. } => {
                        if let ExpressionKind::Identifier(id) = &func.node {
                            self.func_sigs
                                .get(&id.name)
                                .map(|(r, _)| r.clone())
                                .unwrap_or_else(|| "i32".to_string())
                        } else {
                            "i32".to_string()
                        }
                    }
                    _ => "i32".to_string(),
                };

                match op {
                    Operator::AddAssign
                    | Operator::SubAssign
                    | Operator::MulAssign
                    | Operator::DivAssign => {
                        if let ExpressionKind::Identifier(id) = &left.node {
                            let slot = self
                                .locals
                                .get(&id.name)
                                .cloned()
                                .unwrap_or(id.name.clone());
                            let lhs_val = self.next_register();
                            self.lines
                                .push(format!("  {} = load {}, {}* {}", lhs_val, ty, ty, slot));
                            let rhs_val = self.emit_expr(right);
                            let res = self.next_register();
                            let instr = if is_float_ty(&ty) {
                                match op {
                                    Operator::AddAssign => {
                                        format!("  {} = fadd {} {}, {}", res, ty, lhs_val, rhs_val)
                                    }
                                    Operator::SubAssign => {
                                        format!("  {} = fsub {} {}, {}", res, ty, lhs_val, rhs_val)
                                    }
                                    Operator::MulAssign => {
                                        format!("  {} = fmul {} {}, {}", res, ty, lhs_val, rhs_val)
                                    }
                                    Operator::DivAssign => {
                                        format!("  {} = fdiv {} {}, {}", res, ty, lhs_val, rhs_val)
                                    }
                                    _ => {
                                        format!("  {} = fadd {} {}, {}", res, ty, lhs_val, rhs_val)
                                    }
                                }
                            } else {
                                match op {
                                    Operator::AddAssign => {
                                        format!("  {} = add {} {}, {}", res, ty, lhs_val, rhs_val)
                                    }
                                    Operator::SubAssign => {
                                        format!("  {} = sub {} {}, {}", res, ty, lhs_val, rhs_val)
                                    }
                                    Operator::MulAssign => {
                                        format!("  {} = mul {} {}, {}", res, ty, lhs_val, rhs_val)
                                    }
                                    Operator::DivAssign => {
                                        format!("  {} = sdiv {} {}, {}", res, ty, lhs_val, rhs_val)
                                    }
                                    _ => format!("  {} = add {} {}, {}", res, ty, lhs_val, rhs_val),
                                }
                            };
                            self.lines.push(instr);
                            self.lines
                                .push(format!("  store {} {}, {}* {}", ty, res, ty, slot));
                            return res;
                        }
                        if let ExpressionKind::IndexAccess { base, index } = &left.node {
                            let (base_ptr, array_ty) =
                                if let ExpressionKind::Identifier(bid) = &base.node {
                                    let ptr = self
                                        .locals
                                        .get(&bid.name)
                                        .cloned()
                                        .unwrap_or(bid.name.clone());
                                    let aty = self
                                        .locals_type
                                        .get(&bid.name)
                                        .cloned()
                                        .unwrap_or_else(|| "[0 x i32]".to_string());
                                    (ptr, aty)
                                } else {
                                    (self.emit_expr(base), "[0 x i32]".to_string())
                                };
                            let elem_ty = if let Some(xpos) = array_ty.find(" x ") {
                                let after = &array_ty[xpos + 3..];
                                let end = after.find(']').unwrap_or(after.len());
                                after[..end].to_string()
                            } else {
                                "i32".to_string()
                            };
                            let idx_val = self.emit_expr(index);
                            let gep = self.next_register();
                            self.lines.push(format!(
                                "  {} = getelementptr inbounds {}, {}* {}, i32 0, i32 {}",
                                gep, array_ty, array_ty, base_ptr, idx_val
                            ));
                            let lhs_val = self.next_register();
                            self.lines.push(format!(
                                "  {} = load {}, {}* {}",
                                lhs_val, elem_ty, elem_ty, gep
                            ));
                            let rhs_val = self.emit_expr(right);
                            let res = self.next_register();
                            let instr = if is_float_ty(&elem_ty) {
                                match op {
                                    Operator::AddAssign => format!(
                                        "  {} = fadd {} {}, {}",
                                        res, elem_ty, lhs_val, rhs_val
                                    ),
                                    Operator::SubAssign => format!(
                                        "  {} = fsub {} {}, {}",
                                        res, elem_ty, lhs_val, rhs_val
                                    ),
                                    Operator::MulAssign => format!(
                                        "  {} = fmul {} {}, {}",
                                        res, elem_ty, lhs_val, rhs_val
                                    ),
                                    Operator::DivAssign => format!(
                                        "  {} = fdiv {} {}, {}",
                                        res, elem_ty, lhs_val, rhs_val
                                    ),
                                    _ => format!(
                                        "  {} = fadd {} {}, {}",
                                        res, elem_ty, lhs_val, rhs_val
                                    ),
                                }
                            } else {
                                match op {
                                    Operator::AddAssign => format!(
                                        "  {} = add {} {}, {}",
                                        res, elem_ty, lhs_val, rhs_val
                                    ),
                                    Operator::SubAssign => format!(
                                        "  {} = sub {} {}, {}",
                                        res, elem_ty, lhs_val, rhs_val
                                    ),
                                    Operator::MulAssign => format!(
                                        "  {} = mul {} {}, {}",
                                        res, elem_ty, lhs_val, rhs_val
                                    ),
                                    Operator::DivAssign => format!(
                                        "  {} = sdiv {} {}, {}",
                                        res, elem_ty, lhs_val, rhs_val
                                    ),
                                    _ => format!(
                                        "  {} = add {} {}, {}",
                                        res, elem_ty, lhs_val, rhs_val
                                    ),
                                }
                            };
                            self.lines.push(instr);
                            self.lines
                                .push(format!("  store {} {}, {}* {}", elem_ty, res, elem_ty, gep));
                            return res;
                        }
                        if let ExpressionKind::FieldAccess { base, field } = &left.node {
                            let (base_ptr, base_ty) =
                                if let ExpressionKind::Identifier(bid) = &base.node {
                                    let ptr = self
                                        .locals
                                        .get(&bid.name)
                                        .cloned()
                                        .unwrap_or(bid.name.clone());
                                    let bty = self
                                        .locals_type
                                        .get(&bid.name)
                                        .cloned()
                                        .unwrap_or_else(|| "%struct.?".to_string());
                                    (ptr, bty)
                                } else {
                                    (self.emit_expr(base), "%struct.?".to_string())
                                };
                            let struct_name = base_ty.trim_start_matches("%struct.").to_string();
                            let field_index = self
                                .struct_fields
                                .get(&struct_name)
                                .and_then(|fields| fields.iter().position(|f| f.0 == field.name))
                                .unwrap_or(0);
                            let field_ty = self
                                .struct_fields
                                .get(&struct_name)
                                .and_then(|fields| fields.get(field_index))
                                .map(|(_, ty)| ty.clone())
                                .unwrap_or_else(|| "i32".to_string());
                            let gep = self.next_register();
                            self.lines.push(format!(
                                "  {} = getelementptr inbounds {}, {}* {}, i32 0, i32 {}",
                                gep, base_ty, base_ty, base_ptr, field_index
                            ));
                            let lhs_val = self.next_register();
                            self.lines.push(format!(
                                "  {} = load {}, {}* {}",
                                lhs_val, field_ty, field_ty, gep
                            ));
                            let rhs_val = self.emit_expr(right);
                            let res = self.next_register();
                            let instr = if is_float_ty(&field_ty) {
                                match op {
                                    Operator::AddAssign => format!(
                                        "  {} = fadd {} {}, {}",
                                        res, field_ty, lhs_val, rhs_val
                                    ),
                                    Operator::SubAssign => format!(
                                        "  {} = fsub {} {}, {}",
                                        res, field_ty, lhs_val, rhs_val
                                    ),
                                    Operator::MulAssign => format!(
                                        "  {} = fmul {} {}, {}",
                                        res, field_ty, lhs_val, rhs_val
                                    ),
                                    Operator::DivAssign => format!(
                                        "  {} = fdiv {} {}, {}",
                                        res, field_ty, lhs_val, rhs_val
                                    ),
                                    _ => format!(
                                        "  {} = fadd {} {}, {}",
                                        res, field_ty, lhs_val, rhs_val
                                    ),
                                }
                            } else {
                                match op {
                                    Operator::AddAssign => format!(
                                        "  {} = add {} {}, {}",
                                        res, field_ty, lhs_val, rhs_val
                                    ),
                                    Operator::SubAssign => format!(
                                        "  {} = sub {} {}, {}",
                                        res, field_ty, lhs_val, rhs_val
                                    ),
                                    Operator::MulAssign => format!(
                                        "  {} = mul {} {}, {}",
                                        res, field_ty, lhs_val, rhs_val
                                    ),
                                    Operator::DivAssign => format!(
                                        "  {} = sdiv {} {}, {}",
                                        res, field_ty, lhs_val, rhs_val
                                    ),
                                    _ => format!(
                                        "  {} = add {} {}, {}",
                                        res, field_ty, lhs_val, rhs_val
                                    ),
                                }
                            };
                            self.lines.push(instr);
                            self.lines.push(format!(
                                "  store {} {}, {}* {}",
                                field_ty, res, field_ty, gep
                            ));
                            return res;
                        }
                    }
                    Operator::Pipe => {
                        if let ExpressionKind::Call { func, args } = &right.node {
                            if let ExpressionKind::Identifier(id) = &func.node {
                                let mut arg_pairs = Vec::new();
                                let lhs = self.emit_expr(left);
                                let param_types = self
                                    .func_sigs
                                    .get(&id.name)
                                    .map(|(_, p)| p.clone())
                                    .unwrap_or_else(|| vec![]);
                                let first_ty = param_types
                                    .get(0)
                                    .cloned()
                                    .unwrap_or_else(|| "i32".to_string());
                                arg_pairs.push(format!("{} {}", first_ty, lhs));
                                for (idx, a) in args.iter().enumerate() {
                                    let val = self.emit_expr(a);
                                    let ty_a = param_types
                                        .get(idx + 1)
                                        .cloned()
                                        .unwrap_or_else(|| "i32".to_string());
                                    arg_pairs.push(format!("{} {}", ty_a, val));
                                }
                                let ret_ty = self
                                    .func_sigs
                                    .get(&id.name)
                                    .map(|(r, _)| r.clone())
                                    .unwrap_or_else(|| "i32".to_string());
                                let tmp = self.next_register();
                                self.lines.push(format!(
                                    "  {} = call {} @{}({})",
                                    tmp,
                                    ret_ty,
                                    id.name,
                                    arg_pairs.join(", ")
                                ));
                                return tmp;
                            }
                        } else if let ExpressionKind::Identifier(id) = &right.node {
                            let lhs = self.emit_expr(left);
                            let param_types = self
                                .func_sigs
                                .get(&id.name)
                                .map(|(_, p)| p.clone())
                                .unwrap_or_else(|| vec![]);
                            let ty0 = param_types
                                .get(0)
                                .cloned()
                                .unwrap_or_else(|| "i32".to_string());
                            let ret_ty = self
                                .func_sigs
                                .get(&id.name)
                                .map(|(r, _)| r.clone())
                                .unwrap_or_else(|| "i32".to_string());
                            let tmp = self.next_register();
                            self.lines.push(format!(
                                "  {} = call {} @{}({} {})",
                                tmp, ret_ty, id.name, ty0, lhs
                            ));
                            return tmp;
                        }
                        let _ = self.emit_expr(left);
                        let _ = self.emit_expr(right);
                        return "0".to_string();
                    }
                    _ => {}
                }

                let lhs = self.emit_expr(left);
                let rhs = self.emit_expr(right);
                let tmp = self.next_register();
                let instr = if is_float_ty(&ty) {
                    match op {
                        Operator::Add => format!("  {} = fadd {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Sub => format!("  {} = fsub {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Mul => format!("  {} = fmul {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Div => format!("  {} = fdiv {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Mod => format!("  {} = frem {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Eq => format!("  {} = fcmp oeq {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Ne => format!("  {} = fcmp one {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Lt => format!("  {} = fcmp olt {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Gt => format!("  {} = fcmp ogt {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Le => format!("  {} = fcmp ole {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Ge => format!("  {} = fcmp oge {} {}, {}", tmp, ty, lhs, rhs),
                        _ => format!("  {} = fadd {} {}, {}", tmp, ty, lhs, rhs),
                    }
                } else if is_bool_ty(&ty) {
                    match op {
                        Operator::And => format!("  {} = and i1 {}, {}", tmp, lhs, rhs),
                        Operator::Or => format!("  {} = or i1 {}, {}", tmp, lhs, rhs),
                        Operator::Eq => format!("  {} = icmp eq i1 {}, {}", tmp, lhs, rhs),
                        Operator::Ne => format!("  {} = icmp ne i1 {}, {}", tmp, lhs, rhs),
                        _ => format!("  {} = or i1 {}, {}", tmp, lhs, rhs),
                    }
                } else {
                    match op {
                        Operator::Add => format!("  {} = add {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Sub => format!("  {} = sub {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Mul => format!("  {} = mul {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Div => format!("  {} = sdiv {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Mod => format!("  {} = srem {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Eq => format!("  {} = icmp eq {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Ne => format!("  {} = icmp ne {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Lt => format!("  {} = icmp slt {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Gt => format!("  {} = icmp sgt {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Le => format!("  {} = icmp sle {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Ge => format!("  {} = icmp sge {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::And => format!("  {} = and {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Or => format!("  {} = or {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::BitAnd => format!("  {} = and {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::BitOr => format!("  {} = or {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::BitXor => format!("  {} = xor {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Shl => format!("  {} = shl {} {}, {}", tmp, ty, lhs, rhs),
                        Operator::Shr => format!("  {} = lshr {} {}, {}", tmp, ty, lhs, rhs),
                        _ => format!("  {} = add {} {}, {}", tmp, ty, lhs, rhs),
                    }
                };
                self.lines.push(instr);
                tmp
            }
            ExpressionKind::StructInit { name, fields } => {
                let struct_ty = format!("%struct.{}", name.name);
                let tmp = self.next_register();
                self.lines.push(format!("  {} = alloca {}", tmp, struct_ty));
                for (field_name, field_expr) in fields {
                    let field_value = self.emit_expr(field_expr);
                    let field_index = self
                        .struct_fields
                        .get(&name.name)
                        .and_then(|fields| fields.iter().position(|f| f.0 == field_name.name))
                        .unwrap_or(0);
                    let field_type = self
                        .struct_fields
                        .get(&name.name)
                        .and_then(|fields| fields.get(field_index))
                        .map(|(_, ty)| ty.clone())
                        .unwrap_or_else(|| "i32".to_string());
                    let gep = self.next_register();
                    self.lines.push(format!(
                        "  {} = getelementptr inbounds {}, {}* {}, i32 0, i32 {}",
                        gep, struct_ty, struct_ty, tmp, field_index
                    ));
                    self.lines.push(format!(
                        "  store {} {}, {}* {}",
                        field_type, field_value, field_type, gep
                    ));
                }
                tmp
            }
            ExpressionKind::MergeExpression { base, fields } => {
                let (base_ptr, base_ty) = if let Some(base_expr) = base {
                    match &base_expr.node {
                        ExpressionKind::Identifier(id) => {
                            let ptr = self
                                .locals
                                .get(&id.name)
                                .cloned()
                                .unwrap_or_else(|| id.name.clone());
                            let ty = self
                                .locals_type
                                .get(&id.name)
                                .cloned()
                                .unwrap_or_else(|| "%struct.?".to_string());
                            (ptr, ty)
                        }
                        _ => (self.emit_expr(base_expr), "%struct.?".to_string()),
                    }
                } else {
                    ("0".to_string(), "%struct.?".to_string())
                };

                let struct_name = base_ty.trim_start_matches("%struct.").to_string();
                for (field_name, field_expr) in fields {
                    let value = self.emit_expr(field_expr);
                    let field_index = self
                        .struct_fields
                        .get(&struct_name)
                        .and_then(|fields| fields.iter().position(|f| f.0 == field_name.name))
                        .unwrap_or(0);
                    let field_type = self
                        .struct_fields
                        .get(&struct_name)
                        .and_then(|fields| fields.get(field_index))
                        .map(|(_, ty)| ty.clone())
                        .unwrap_or_else(|| "i32".to_string());
                    let gep = self.next_register();
                    self.lines.push(format!(
                        "  {} = getelementptr inbounds {}, {}* {}, i32 0, i32 {}",
                        gep, base_ty, base_ty, base_ptr, field_index
                    ));
                    self.lines.push(format!(
                        "  store {} {}, {}* {}",
                        field_type, value, field_type, gep
                    ));
                }
                base_ptr
            }
            ExpressionKind::FieldAccess { base, field } => {
                let (base_ptr, base_ty) = match &base.node {
                    ExpressionKind::Identifier(id) => {
                        let ptr = self
                            .locals
                            .get(&id.name)
                            .cloned()
                            .unwrap_or(id.name.clone());
                        let ty = self
                            .locals_type
                            .get(&id.name)
                            .cloned()
                            .unwrap_or_else(|| "%struct.?".to_string());
                        (ptr, ty)
                    }
                    ExpressionKind::StructInit { name, .. } => {
                        let ptr = self.emit_expr(base);
                        let ty = format!("%struct.{}", name.name);
                        (ptr, ty)
                    }
                    _ => (self.emit_expr(base), "%struct.?".to_string()),
                };
                let struct_name = base_ty.trim_start_matches("%struct.").to_string();
                let field_index = self
                    .struct_fields
                    .get(&struct_name)
                    .and_then(|fields| fields.iter().position(|f| f.0 == field.name))
                    .unwrap_or(0);
                let field_ty = self
                    .struct_fields
                    .get(&struct_name)
                    .and_then(|fields| fields.get(field_index))
                    .map(|(_, ty)| ty.clone())
                    .unwrap_or_else(|| "i32".to_string());
                let gep = self.next_register();
                self.lines.push(format!(
                    "  {} = getelementptr inbounds {}, {}* {}, i32 0, i32 {}",
                    gep, base_ty, base_ty, base_ptr, field_index
                ));
                let tmp = self.next_register();
                self.lines.push(format!(
                    "  {} = load {}, {}* {}",
                    tmp, field_ty, field_ty, gep
                ));
                tmp
            }
            ExpressionKind::IndexAccess { base, index } => {
                let (base_ptr, array_ty) = match &base.node {
                    ExpressionKind::Identifier(id) => {
                        let ptr = self
                            .locals
                            .get(&id.name)
                            .cloned()
                            .unwrap_or(id.name.clone());
                        let aty = self
                            .locals_type
                            .get(&id.name)
                            .cloned()
                            .unwrap_or_else(|| "[0 x i32]".to_string());
                        (ptr, aty)
                    }
                    _ => (self.emit_expr(base), "[0 x i32]".to_string()),
                };
                let elem_ty = if let Some(xpos) = array_ty.find(" x ") {
                    let after = &array_ty[xpos + 3..];
                    let end = after.find(']').unwrap_or(after.len());
                    after[..end].to_string()
                } else {
                    "i32".to_string()
                };
                let idx_val = self.emit_expr(index);
                let gep = self.next_register();
                self.lines.push(format!(
                    "  {} = getelementptr inbounds {}, {}* {}, i32 0, i32 {}",
                    gep, array_ty, array_ty, base_ptr, idx_val
                ));
                let tmp = self.next_register();
                self.lines.push(format!(
                    "  {} = load {}, {}* {}",
                    tmp, elem_ty, elem_ty, gep
                ));
                tmp
            }
            ExpressionKind::Call { func, args } => {
                if let ExpressionKind::Identifier(id) = &func.node {
                    let (ret_ty, param_types) = match self.func_sigs.get(&id.name) {
                        Some(sig) => sig.clone(),
                        None => ("i32".to_string(), vec![]),
                    };
                    let mut arg_pairs = Vec::new();
                    for (idx, arg) in args.iter().enumerate() {
                        let val = self.emit_expr(arg);
                        let ty = param_types
                            .get(idx)
                            .cloned()
                            .unwrap_or_else(|| "i32".to_string());
                        arg_pairs.push(format!("{} {}", ty, val));
                    }
                    let tmp = self.next_register();
                    self.lines.push(format!(
                        "  {} = call {} @{}({})",
                        tmp,
                        ret_ty,
                        id.name,
                        arg_pairs.join(", ")
                    ));
                    return tmp;
                }
                "0".to_string()
            }
            ExpressionKind::TryOperator { expr } => self.emit_expr(expr),
            ExpressionKind::Match { expr, arms } => {
                let _ = self.emit_expr(expr);
                for arm in arms {
                    let _ = self.emit_expr(&arm.node.body);
                }
                "0".to_string()
            }
            _ => "0".to_string(),
        }
    }

    fn next_register(&mut self) -> String {
        let name = format!("%t{}", self.next_tmp);
        self.next_tmp += 1;
        name
    }

    fn lower_type(&self, ty: &Type) -> String {
        match ty.node.name.as_str() {
            "Int8" => "i8".to_string(),
            "Int16" => "i16".to_string(),
            "Int32" => "i32".to_string(),
            "Int64" => "i64".to_string(),
            "Float16" => "half".to_string(),
            "Float32" => "float".to_string(),
            "Float64" => "double".to_string(),
            "Bool" => "i1".to_string(),
            "Str" => "i8*".to_string(),
            "Buf" => "%struct.Buf*".to_string(),
            "ref" => "i8*".to_string(),
            "Char" => "i8".to_string(),
            "Byte" => "i8".to_string(),
            name => format!("%struct.{}", name),
        }
    }

    fn next_block(&mut self, prefix: &str) -> String {
        let label = format!("{}_{}", prefix, self.next_label);
        self.next_label += 1;
        label
    }
}

impl IrModule {
    pub fn to_llvm_ir(&self) -> String {
        let mut out = Vec::new();
        out.extend(self.preamble.iter().cloned());
        for func in &self.functions {
            let params = if func.params.is_empty() {
                "".to_string()
            } else {
                func.params
                    .iter()
                    .map(|(name, ty)| format!("{} %{}", ty, name))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push(format!(
                "define {} @{}({}) {{",
                func.ret_type, func.name, params
            ));
            out.push(func.body.clone());
            out.push("}".to_string());
        }
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

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

    fn parse_module(source: &str) -> Module {
        Parser::new(Lexer::new(normalize_source(source)).tokenize())
            .parse_module()
            .unwrap()
    }

    #[test]
    fn lowers_function_declarations() {
        let source = "fn main(a: Int32) -> Int32 { return a; }";
        let module = parse_module(source);
        let ir = Codegen::lower_module(&module);
        assert_eq!(ir.functions.len(), 1);
        assert_eq!(ir.functions[0].name, "main");
        assert_eq!(ir.functions[0].ret_type, "i32");
        assert_eq!(ir.functions[0].params.len(), 1);
    }

    #[test]
    fn emits_basic_llvm_ir() {
        let source = "fn main() -> Int32 { return 0; }";
        let module = parse_module(source);
        let ir = Codegen::lower_module(&module);
        let text = ir.to_llvm_ir();
        assert!(text.contains("define i32 @main()"));
        assert!(text.contains("ret i32 0"));
    }

    #[test]
    fn lowers_let_bindings() {
        let source = "fn main() -> Int32 { let x: Int32 = 3; return x; }";
        let module = parse_module(source);
        let ir = Codegen::lower_module(&module);
        let text = ir.to_llvm_ir();
        assert!(text.contains("alloca i32"));
        assert!(text.contains("store i32 3"));
        assert!(text.contains("load i32"));
    }

    #[test]
    fn emits_if_branches() {
        let source = "fn main(flag: Bool) -> Int32 { if flag { return 1; } else { return 2; } }";
        let module = parse_module(source);
        let ir = Codegen::lower_module(&module);
        let text = ir.to_llvm_ir();
        assert!(text.contains("br i1"));
        assert!(text.contains("if_merge"));
    }

    #[test]
    fn lowers_struct_init_and_merge() {
        let source = "struct User { id: Int32, age: Int32 } fn main() -> Int32 { let user: User = User { id: 1, age: 2 }; let updated: User = { ...user, age: 3 }; return 0; }";
        let module = parse_module(source);
        let ir = Codegen::lower_module(&module);
        let text = ir.to_llvm_ir();
        assert!(text.contains("%struct.User = type"));
        assert!(text.contains("getelementptr"));
        assert!(text.contains("store i32 1"));
        assert!(text.contains("store i32 3"));
    }
}
