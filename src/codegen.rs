use crate::ast::*;
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
            if let Declaration::Function {
                name,
                return_type,
                body,
                params,
                ..
            } = decl
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
            match decl {
                Declaration::Struct { name, fields, .. } => {
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
                Declaration::Enum { name, .. } => {
                    self.type_decls
                        .push(format!("%enum.{} = type opaque", name.name));
                }
                Declaration::Newtype { name, type_alias } => {
                    let alias = self.lower_type(type_alias);
                    self.type_decls
                        .push(format!("%newtype.{} = type {}", name.name, alias));
                }
                Declaration::Function {
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
        // If any explicit 'ret' was already emitted in the function body,
        // don't append a default return — avoid incorrect duplicate returns.
        let has_ret = self
            .lines
            .iter()
            .any(|l| l.trim_start().starts_with("ret ") || l.trim_start().starts_with("ret\t"));

        if let Some(value) = self.last_value.take() {
            if return_type != "void" {
                // If a ret already exists, skip appending here.
                if !has_ret {
                    self.lines.push(format!("  ret {} {}", return_type, value));
                }
            } else if !has_ret {
                self.lines.push("  ret void".to_string());
            }
        } else if !has_ret {
            // No expression produced a value and no ret emitted; append void ret.
            self.lines.push("  ret void".to_string());
        }

        // Ensure the final basic block is properly terminated. If the last line
        // is a label (ends with ':'), append a default return so the block has
        // a terminator. This handles cases where nested branches produced some
        // 'ret' instructions but the fall-through path still needs a return.
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
        for stmt in &block.statements {
            match stmt {
                Statement::Return(Some(expr)) => {
                    let value = self.emit_expr(expr);
                    self.last_value = Some(value);
                    return;
                }
                Statement::Return(None) => {
                    self.last_value = None;
                    return;
                }
                Statement::LetBinding {
                    name,
                    initializer,
                    type_annotation,
                    ..
                } => {
                    // Special-case array literal initializers so we allocate a stack array
                    if let Expression::Literal(Literal::Array(elems)) = initializer {
                        // All elements are assumed i32 for now
                        let len = elems.len();
                        let array_ty = format!("[{} x i32]", len);
                        let alloca = self.next_register();
                        self.lines
                            .push(format!("  {} = alloca {}", alloca, array_ty));
                        for (i, elem_expr) in elems.iter().enumerate() {
                            let val = match elem_expr {
                                Expression::Literal(Literal::Int(v)) => v.to_string(),
                                _ => self.emit_expr(elem_expr),
                            };
                            let gep = self.next_register();
                            self.lines.push(format!(
                                "  {} = getelementptr inbounds {}, {}* {}, i32 0, i32 {}",
                                gep, array_ty, array_ty, alloca, i
                            ));
                            self.lines
                                .push(format!("  store i32 {}, i32* {}", val, gep));
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
                Statement::Expression(expr) => {
                    let _ = self.emit_expr(expr);
                }
                Statement::If {
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

                    // ── then ─────────────────────────────────────────────────────────────
                    self.lines.push(format!("{}:", then_label));
                    let then_terminated =
                        self.emit_block_terminated(then_branch, current_fn_ret_ty);
                    if !then_terminated {
                        self.lines.push(format!("  br label %{}", merge_label));
                    }

                    // ── else ─────────────────────────────────────────────────────────────
                    self.lines.push(format!("{}:", else_label));
                    let else_terminated = match else_branch {
                        None => {
                            self.lines.push(format!("  br label %{}", merge_label));
                            true
                        }
                        Some(ElseBranch::Block(block)) => {
                            let t = self.emit_block_terminated(block, current_fn_ret_ty);
                            if !t {
                                self.lines.push(format!("  br label %{}", merge_label));
                            }
                            t
                        }
                        Some(ElseBranch::If(stmt)) => {
                            // else if recurses — emit_statement_terminated handles If,
                            // which will emit its own inner merge block if needed.
                            // The outer else_label becomes the entry of the nested if.
                            self.emit_statement_terminated(stmt, current_fn_ret_ty)
                        }
                    };

                    // ── merge ─────────────────────────────────────────────────────────────
                    // Always emit a merge label for consistency with tests. If both
                    // branches already terminate, emit an 'unreachable' terminator
                    // so the label is syntactically valid for LLVM.
                    self.lines.push(format!("{}:", merge_label));
                    if then_terminated && else_terminated {
                        self.lines.push("  unreachable".to_string());
                    }
                }
                Statement::Match { expr, arms } => {
                    let discr = self.emit_expr(expr);
                    let merge_label = self.next_block("match_merge");
                    for (idx, arm) in arms.iter().enumerate() {
                        let arm_label = self.next_block(&format!("match_arm_{}", idx));
                        self.lines.push(format!("  br label %{}", arm_label));
                        self.lines.push(format!("{}:", arm_label));
                        let _ = self.emit_expr(&arm.body);
                        self.lines.push(format!("  br label %{}", merge_label));
                    }
                    self.lines.push(format!("{}:", merge_label));
                    let _ = discr;
                }
                Statement::Loop { kind, body } => {
                    let loop_label = self.next_block("loop");
                    let loop_body = self.next_block("loop_body");
                    let loop_end = self.next_block("loop_end");
                    match kind {
                        LoopKind::While { condition, .. } => {
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
                        LoopKind::For {
                            pattern,
                            iterator,
                            body: _,
                        } => {
                            // Only handle simple `for x in arr` where `arr` is a local stack array `[N x i32]`
                            if let Expression::Identifier(iter_id) = iterator {
                                if let Some(iter_ty) = self.locals_type.get(&iter_id.name).cloned()
                                {
                                    if iter_ty.starts_with('[') && iter_ty.contains(" x i32") {
                                        // parse length
                                        if let Some(end_bracket) = iter_ty.find(']') {
                                            // format: [N x i32]
                                            let inside = &iter_ty[1..end_bracket];
                                            if let Some(space_idx) = inside.find(' ') {
                                                let len_str = &inside[..space_idx];
                                                if let Ok(len) = len_str.parse::<usize>() {
                                                    if let Pattern::Identifier(ident) = pattern {
                                                        // allocate pattern slot and index slot BEFORE emitting the loop labels
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
                                                        // Now emit the loop control
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
                                                        // Emit body label and load element for this iteration
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
                                                        // Emit the loop body, detect if it terminates
                                                        let body_terminated = self
                                                            .emit_block_terminated(
                                                                body,
                                                                current_fn_ret_ty,
                                                            );
                                                        if !body_terminated {
                                                            // increment idx and loop back
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
                                                        // Emit loop_end label (always define it so branch targets exist)
                                                        self.lines.push(format!("{}:", loop_end));
                                                        // Done lowering this for-loop
                                                        continue;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            // Fallback: simple loop if cannot lower
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
                Statement::Conc { body } => {
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

    /// Emits a block and returns `true` if the block ended with a terminator
    /// (ret or br), so the caller knows whether a fall-through br is needed.
    fn emit_block_terminated(&mut self, block: &Block, ret_ty: &str) -> bool {
        for stmt in &block.statements {
            let terminated = self.emit_statement_terminated(stmt, ret_ty);
            if terminated {
                // A terminator was emitted mid-block; remaining statements
                // would be unreachable — stop here.
                return true;
            }
        }
        false // block fell off the end with no terminator
    }

    /// Emits a single statement and returns `true` if it produced a terminator.
    fn emit_statement_terminated(&mut self, stmt: &Statement, ret_ty: &str) -> bool {
        match stmt {
            Statement::Return(Some(expr)) => {
                let value = self.emit_expr(expr);
                self.lines.push(format!("  ret {} {}", ret_ty, value));
                true
            }
            Statement::Return(None) => {
                self.lines.push("  ret void".to_string());
                true
            }
            Statement::If { condition, then_branch, else_branch } => {
                let cond_val = self.emit_expr(condition);
                let then_label = self.next_block("then");
                let else_label = self.next_block("else");
                let merge_label = self.next_block("if_merge");

                self.lines.push(format!(
                    "  br i1 {}, label %{}, label %{}",
                    cond_val, then_label, else_label
                ));

                // then
                self.lines.push(format!("{}:", then_label));
                let then_terminated = self.emit_block_terminated(then_branch, ret_ty);
                if !then_terminated {
                    self.lines.push(format!("  br label %{}", merge_label));
                }

                // else
                self.lines.push(format!("{}:", else_label));
                let else_terminated = match else_branch {
                    None => {
                        self.lines.push(format!("  br label %{}", merge_label));
                        true
                    }
                    Some(ElseBranch::Block(block)) => {
                        let t = self.emit_block_terminated(block, ret_ty);
                        if !t {
                            self.lines.push(format!("  br label %{}", merge_label));
                        }
                        t
                    }
                    Some(ElseBranch::If(stmt)) => self.emit_statement_terminated(stmt, ret_ty),
                };

                // merge
                self.lines.push(format!("{}:", merge_label));
                if then_terminated && else_terminated {
                    self.lines.push("  unreachable".to_string());
                    return true;
                }
                false
            }
            Statement::Match { expr, arms } => {
                // Simple lowering: emit each arm into its own block and branch to merge.
                let merge_label = self.next_block("match_merge");
                for (idx, arm) in arms.iter().enumerate() {
                    let arm_label = self.next_block(&format!("match_arm_{}", idx));
                    self.lines.push(format!("  br label %{}", arm_label));
                    self.lines.push(format!("{}:", arm_label));
                    let _ = self.emit_expr(&arm.body);
                    self.lines.push(format!("  br label %{}", merge_label));
                }
                self.lines.push(format!("{}:", merge_label));
                false
            }
            Statement::Loop { kind, body } => {
                // Emit loop but assume it does not terminate the enclosing block.
                match kind {
                    LoopKind::While { condition, .. } => {
                        let loop_label = self.next_block("loop");
                        let loop_body = self.next_block("loop_body");
                        let loop_end = self.next_block("loop_end");
                        self.lines.push(format!("  br label %{}", loop_label));
                        self.lines.push(format!("{}:", loop_label));
                        let cond_val = self.emit_expr(condition);
                        self.lines.push(format!("  br i1 {}, label %{}, label %{}", cond_val, loop_body, loop_end));
                        self.lines.push(format!("{}:", loop_body));
                        self.emit_block(body, ret_ty);
                        self.lines.push(format!("  br label %{}", loop_label));
                        self.lines.push(format!("{}:", loop_end));
                    }
                    LoopKind::For { .. } => {
                        // Delegate to emit_statement (which will call emit_block)
                        self.emit_statement(stmt, ret_ty);
                    }
                    _ => {
                        self.emit_statement(stmt, ret_ty);
                    }
                }
                false
            }
            // Delegate everything else to existing emit_statement; assume no terminator
            other => {
                self.emit_statement(other, ret_ty);
                false
            }
        }
    }

    fn emit_statement(&mut self, stmt: &Statement, current_fn_ret_ty: &str) {
        match stmt {
            Statement::Return(Some(expr)) => {
                let value = self.emit_expr(expr);
                self.last_value = Some(value);
            }
            Statement::Return(None) => {
                self.last_value = None;
            }
            Statement::Expression(expr) => {
                let _ = self.emit_expr(expr);
            }
            Statement::If { .. } | Statement::Match { .. } | Statement::Loop { .. } => {
                self.emit_block(
                    &Block {
                        statements: vec![stmt.clone()],
                        trailing_expression: None,
                    },
                    current_fn_ret_ty,
                );
            }
            _ => {}
        }
    }

    fn emit_expr(&mut self, expr: &Expression) -> String {
        match expr {
            Expression::Literal(Literal::Int(value)) => value.to_string(),
            Expression::Literal(Literal::Bool(value)) => {
                if *value {
                    "1".to_string()
                } else {
                    "0".to_string()
                }
            }
            Expression::Identifier(id) => {
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
            Expression::BinaryOp { op, left, right } => {
                let lhs = self.emit_expr(left);
                let rhs = self.emit_expr(right);
                let tmp = self.next_register();
                let instr = match op {
                    Operator::Add => format!("  {} = add i32 {}, {}", tmp, lhs, rhs),
                    Operator::Sub => format!("  {} = sub i32 {}, {}", tmp, lhs, rhs),
                    Operator::Mul => format!("  {} = mul i32 {}, {}", tmp, lhs, rhs),
                    Operator::Div => format!("  {} = sdiv i32 {}, {}", tmp, lhs, rhs),
                    Operator::Mod => format!("  {} = srem i32 {}, {}", tmp, lhs, rhs),
                    Operator::Eq => format!("  {} = icmp eq i32 {}, {}", tmp, lhs, rhs),
                    Operator::Ne => format!("  {} = icmp ne i32 {}, {}", tmp, lhs, rhs),
                    Operator::Lt => format!("  {} = icmp slt i32 {}, {}", tmp, lhs, rhs),
                    Operator::Gt => format!("  {} = icmp sgt i32 {}, {}", tmp, lhs, rhs),
                    Operator::Le => format!("  {} = icmp sle i32 {}, {}", tmp, lhs, rhs),
                    Operator::Ge => format!("  {} = icmp sge i32 {}, {}", tmp, lhs, rhs),
                    Operator::And => format!("  {} = and i1 {}, {}", tmp, lhs, rhs),
                    Operator::Or => format!("  {} = or i1 {}, {}", tmp, lhs, rhs),
                    Operator::BitAnd => format!("  {} = and i32 {}, {}", tmp, lhs, rhs),
                    Operator::BitOr => format!("  {} = or i32 {}, {}", tmp, lhs, rhs),
                    Operator::BitXor => format!("  {} = xor i32 {}, {}", tmp, lhs, rhs),
                    Operator::Shl => format!("  {} = shl i32 {}, {}", tmp, lhs, rhs),
                    Operator::Shr => format!("  {} = lshr i32 {}, {}", tmp, lhs, rhs),
                    _ => format!("  {} = add i32 {}, {}", tmp, lhs, rhs),
                };
                self.lines.push(instr);
                tmp
            }
            Expression::StructInit { name, fields } => {
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
            Expression::MergeExpression { base, fields } => {
                let (base_ptr, base_ty) = if let Some(Expression::Identifier(id)) = base.as_deref()
                {
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
                } else if let Some(base_expr) = base {
                    (self.emit_expr(base_expr), "%struct.?".to_string())
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
            Expression::Call { func, args } => {
                if let Expression::Identifier(id) = func.as_ref() {
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
            Expression::Match { expr, arms } => {
                let _ = self.emit_expr(expr);
                for arm in arms {
                    let _ = self.emit_expr(&arm.body);
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
        match ty.name.as_str() {
            "Int32" => "i32".to_string(),
            "Bool" => "i1".to_string(),
            "Str" => "i8*".to_string(),
            "Buf" => "%struct.Buf*".to_string(),
            "ref" => "i8*".to_string(),
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
