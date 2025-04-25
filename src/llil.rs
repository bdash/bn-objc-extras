//! This module provides wrappers around Binary Ninja's `low_level_il` types to simplify
//! pattern matching and make it more ergonomic to work with.

#![allow(unused)]
use binaryninja::{
    architecture,
    high_level_il::operation,
    low_level_il::{
        self,
        expression::{ExpressionHandler, LowLevelILExpression, ValueExpr},
        instruction::{InstructionHandler, LowLevelILInstruction},
    },
};

#[derive(Debug)]
pub(crate) enum Instruction<'a, A, M, F>
where
    A: 'a + architecture::Architecture,
    M: low_level_il::function::FunctionMutability,
    F: low_level_il::function::FunctionForm,
{
    If(
        Expression<'a, A, M, F>,
        LowLevelILInstruction<'a, A, M, F>,
        LowLevelILInstruction<'a, A, M, F>,
    ),
    SetReg(
        low_level_il::LowLevelILRegister<A::Register>,
        Expression<'a, A, M, F>,
    ),
    Call(Expression<'a, A, M, F>),
    TailCall(Expression<'a, A, M, F>),
    Goto(LowLevelILInstruction<'a, A, M, F>),
    Jump(Expression<'a, A, M, F>),
    Unknown(&'a LowLevelILInstruction<'a, A, M, F>),
}

#[derive(Debug)]
pub(crate) struct BinaryExpression<'a, A, M, F>(
    pub Expression<'a, A, M, F>,
    pub Expression<'a, A, M, F>,
)
where
    A: 'a + architecture::Architecture,
    M: low_level_il::function::FunctionMutability,
    F: low_level_il::function::FunctionForm;

#[derive(Debug)]
pub(crate) enum Expression<'a, A, M, F>
where
    A: architecture::Architecture,
    M: low_level_il::function::FunctionMutability,
    F: low_level_il::function::FunctionForm,
{
    And(Box<BinaryExpression<'a, A, M, F>>),
    Xor(Box<BinaryExpression<'a, A, M, F>>),
    Lsl(Box<BinaryExpression<'a, A, M, F>>),
    CmpE(Box<BinaryExpression<'a, A, M, F>>),
    Reg(low_level_il::LowLevelILRegister<A::Register>),
    Const(u64),
    ConstPtr(u64),
    Unknown(LowLevelILExpression<'a, A, M, F, ValueExpr>),
}

impl<'a, A, M, F> From<&'a LowLevelILInstruction<'a, A, M, F>> for Instruction<'a, A, M, F>
where
    A: 'a + architecture::Architecture,
    M: low_level_il::function::FunctionMutability,
    F: low_level_il::function::FunctionForm,
    LowLevelILInstruction<'a, A, M, F>: low_level_il::instruction::InstructionHandler<'a, A, M, F>,
    LowLevelILExpression<'a, A, M, F, ValueExpr>:
        low_level_il::expression::ExpressionHandler<'a, A, M, F>,
{
    fn from(instr: &'a LowLevelILInstruction<'a, A, M, F>) -> Self {
        use low_level_il::instruction::LowLevelILInstructionKind as Kind;
        match instr.kind() {
            Kind::If(operation) => Self::If(
                operation.condition().into(),
                operation.true_target(),
                operation.false_target(),
            ),
            Kind::SetReg(operation) => {
                Instruction::SetReg(operation.dest_reg(), operation.source_expr().into())
            }
            Kind::Call(operation) => Instruction::Call(operation.target().into()),
            Kind::TailCall(operation) => Instruction::TailCall(operation.target().into()),
            Kind::Goto(operation) => Instruction::Goto(operation.target()),
            Kind::Jump(operation) => Instruction::Jump(operation.target().into()),
            _ => Instruction::Unknown(instr),
        }
    }
}

impl<'a, A, M, F> From<LowLevelILExpression<'a, A, M, F, ValueExpr>> for Expression<'a, A, M, F>
where
    A: 'a + architecture::Architecture,
    M: low_level_il::function::FunctionMutability,
    F: low_level_il::function::FunctionForm,
    LowLevelILExpression<'a, A, M, F, ValueExpr>:
        low_level_il::expression::ExpressionHandler<'a, A, M, F>,
{
    fn from(expr: LowLevelILExpression<'a, A, M, F, ValueExpr>) -> Self {
        use low_level_il::expression::LowLevelILExpressionKind as Kind;
        match expr.kind() {
            Kind::And(operation) => Expression::And(Box::new(BinaryExpression::from(operation))),
            Kind::Xor(operation) => Expression::Xor(Box::new(BinaryExpression::from(operation))),
            Kind::Lsl(operation) => Expression::Lsl(Box::new(BinaryExpression::from(operation))),
            Kind::CmpE(operation) => Expression::CmpE(Box::new(BinaryExpression(
                operation.left().into(),
                operation.right().into(),
            ))),
            Kind::Const(operation) => Expression::Const(operation.value()),
            Kind::ConstPtr(operation) => Expression::ConstPtr(operation.value()),
            Kind::Reg(operation) => Expression::Reg(operation.source_reg()),
            _ => Expression::Unknown(expr),
        }
    }
}

impl<'a, A, M, F>
    From<low_level_il::operation::Operation<'a, A, M, F, low_level_il::operation::BinaryOp>>
    for BinaryExpression<'a, A, M, F>
where
    A: 'a + architecture::Architecture,
    M: low_level_il::function::FunctionMutability,
    F: low_level_il::function::FunctionForm,
    LowLevelILExpression<'a, A, M, F, ValueExpr>: ExpressionHandler<'a, A, M, F>,
{
    fn from(
        operation: low_level_il::operation::Operation<
            'a,
            A,
            M,
            F,
            low_level_il::operation::BinaryOp,
        >,
    ) -> Self {
        Self(operation.left().into(), operation.right().into())
    }
}
