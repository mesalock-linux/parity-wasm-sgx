// TODO: remove this
#![allow(unused)]

use std::rc::Rc;
use std::cell::{Cell, RefCell};
use std::any::Any;
use std::fmt;
use std::collections::HashMap;
use elements::{FunctionType, GlobalEntry, GlobalType, InitExpr, Internal, External, Local, MemoryType,
               Module, Opcode, Opcodes, TableType, Type, ResizableLimits};
use interpreter::{Error, ExecutionParams, MemoryInstance,
                  RuntimeValue, TableInstance};
use interpreter::runner::{prepare_function_args, FunctionContext, Interpreter};
use interpreter::host::AnyFunc;
use validation::validate_module;
use common::{DEFAULT_FRAME_STACK_LIMIT, DEFAULT_MEMORY_INDEX, DEFAULT_TABLE_INDEX,
             DEFAULT_VALUE_STACK_LIMIT};
use common::stack::StackWithLimit;

#[derive(Clone, Debug)]
pub enum ExternVal {
	Func(Rc<FuncInstance>),
	Table(Rc<TableInstance>),
	Memory(Rc<MemoryInstance>),
	Global(Rc<GlobalInstance>),
}

impl ExternVal {
	pub fn as_func(&self) -> Option<Rc<FuncInstance>> {
		match *self {
			ExternVal::Func(ref func) => Some(Rc::clone(func)),
			_ => None,
		}
	}

	pub fn as_table(&self) -> Option<Rc<TableInstance>> {
		match *self {
			ExternVal::Table(ref table) => Some(Rc::clone(table)),
			_ => None,
		}
	}

	pub fn as_memory(&self) -> Option<Rc<MemoryInstance>> {
		match *self {
			ExternVal::Memory(ref memory) => Some(Rc::clone(memory)),
			_ => None,
		}
	}

	pub fn as_global(&self) -> Option<Rc<GlobalInstance>> {
		match *self {
			ExternVal::Global(ref global) => Some(Rc::clone(global)),
			_ => None,
		}
	}
}

#[derive(Clone)]
pub enum FuncInstance {
	Internal {
		func_type: Rc<FunctionType>,
		module: Rc<ModuleInstance>,
		body: Rc<FuncBody>,
	},
	Host {
		func_type: Rc<FunctionType>,
		host_func: Rc<AnyFunc>,
	},
}

impl fmt::Debug for FuncInstance {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match self {
			&FuncInstance::Internal { ref func_type, ref module, .. } => {
				write!(f, "Internal {{ type={:?}, module={:?} }}", func_type, module)
			},
			&FuncInstance::Host { ref func_type, .. } => {
				write!(f, "Host {{ type={:?} }}", func_type)
			}
		}
	}
}

impl FuncInstance {
	pub fn func_type(&self) -> Rc<FunctionType> {
		match *self {
			FuncInstance::Internal { ref func_type, .. } | FuncInstance::Host { ref func_type, .. } => {
				Rc::clone(func_type)
			}
		}
	}

	pub fn body(&self) -> Option<Rc<FuncBody>> {
		match *self {
			FuncInstance::Internal { ref body, .. } => Some(Rc::clone(body)),
			FuncInstance::Host { .. } => None,
		}
	}
}

#[derive(Clone, Debug)]
pub struct FuncBody {
	pub locals: Vec<Local>,
	pub opcodes: Opcodes,
	pub labels: HashMap<usize, usize>,
}

#[derive(Debug)]
pub struct GlobalInstance {
	val: Cell<RuntimeValue>,
	mutable: bool,
}

impl GlobalInstance {
	pub fn new(val: RuntimeValue, mutable: bool) -> GlobalInstance {
		GlobalInstance {
			val: Cell::new(val),
			mutable
		}
	}

	pub fn set(&self, val: RuntimeValue) -> Result<(), Error> {
		if !self.mutable {
			// TODO: better error message
			return Err(Error::Validation("Can't set immutable global".into()));
		}
		self.val.set(val);
		Ok(())
	}

	pub fn get(&self) -> RuntimeValue {
		self.val.get()
	}
}

pub struct ExportInstance {
	name: String,
	val: ExternVal,
}

#[derive(Default, Debug)]
pub struct ModuleInstance {
	types: RefCell<Vec<Rc<FunctionType>>>,
	tables: RefCell<Vec<Rc<TableInstance>>>,
	funcs: RefCell<Vec<Rc<FuncInstance>>>,
	memories: RefCell<Vec<Rc<MemoryInstance>>>,
	globals: RefCell<Vec<Rc<GlobalInstance>>>,
	exports: RefCell<HashMap<String, ExternVal>>,
}

impl ModuleInstance {
	fn new() -> ModuleInstance {
		ModuleInstance::default()
	}

	pub fn with_exports(exports: HashMap<String, ExternVal>) -> ModuleInstance {
		ModuleInstance {
			exports: RefCell::new(exports), ..Default::default()
		}
	}

	pub fn memory_by_index(&self, idx: u32) -> Option<Rc<MemoryInstance>> {
		self
			.memories
			.borrow()
			.get(idx as usize)
			.cloned()
	}

	pub fn table_by_index(&self, idx: u32) -> Option<Rc<TableInstance>> {
		self
			.tables
			.borrow()
			.get(idx as usize)
			.cloned()
	}

	pub fn global_by_index(&self, idx: u32) -> Option<Rc<GlobalInstance>> {
		self
			.globals
			.borrow()
			.get(idx as usize)
			.cloned()
	}

	pub fn func_by_index(&self, idx: u32) -> Option<Rc<FuncInstance>> {
		self
			.funcs
			.borrow()
			.get(idx as usize)
			.cloned()
	}

	pub fn type_by_index(&self, idx: u32) -> Option<Rc<FunctionType>> {
		self
			.types
			.borrow()
			.get(idx as usize)
			.cloned()
	}

	pub fn export_by_name(&self, name: &str) -> Option<ExternVal> {
		self
			.exports
			.borrow()
			.get(name)
			.cloned()
	}

	fn push_func(&self, func: Rc<FuncInstance>) {
		self.funcs.borrow_mut().push(func);
	}

	fn push_type(&self, func_type: Rc<FunctionType>) {
		self.types.borrow_mut().push(func_type)
	}

	fn push_memory(&self, memory: Rc<MemoryInstance>) {
		self.memories.borrow_mut().push(memory)
	}

	fn push_table(&self, table: Rc<TableInstance>) {
		self.tables.borrow_mut().push(table)
	}

	fn push_global(&self, global: Rc<GlobalInstance>) {
		self.globals.borrow_mut().push(global)
	}

	fn insert_export<N: Into<String>>(&self, name: N, extern_val: ExternVal) {
		self
			.exports
			.borrow_mut()
			.insert(name.into(), extern_val);
	}
}

#[derive(Default, Debug)]
pub struct Store;

impl Store {
	pub fn new() -> Store {
		Store::default()
	}

	pub fn alloc_func_type(&mut self, func_type: FunctionType) -> Rc<FunctionType> {
		Rc::new(func_type)
	}

	pub fn alloc_func(&mut self, module: &Rc<ModuleInstance>, func_type: Rc<FunctionType>, body: FuncBody) -> Rc<FuncInstance> {
		let func = FuncInstance::Internal {
			func_type,
			module: Rc::clone(module),
			body: Rc::new(body),
		};
		Rc::new(func)
	}

	pub fn alloc_host_func(&mut self, func_type: Rc<FunctionType>, host_func: Rc<AnyFunc>) -> Rc<FuncInstance> {
		let func = FuncInstance::Host {
			func_type,
			host_func,
		};
		Rc::new(func)
	}

	pub fn alloc_table(&mut self, table_type: &TableType) -> Result<Rc<TableInstance>, Error> {
		let table = TableInstance::new(table_type)?;
		Ok(Rc::new(table))
	}

	pub fn alloc_memory(&mut self, mem_type: &MemoryType) -> Result<Rc<MemoryInstance>, Error> {
		let memory = MemoryInstance::new(&mem_type)?;
		Ok(Rc::new(memory))
	}

	pub fn alloc_global(&mut self, global_type: GlobalType, val: RuntimeValue) -> Rc<GlobalInstance> {
		let global = GlobalInstance::new(val, global_type.is_mutable());
		Rc::new(global)
	}

	fn alloc_module_internal(
		&mut self,
		module: &Module,
		extern_vals: &[ExternVal],
		instance: &Rc<ModuleInstance>,
	) -> Result<(), Error> {
		let mut aux_data = validate_module(module)?;

		for &Type::Function(ref ty) in module
			.type_section()
			.map(|ts| ts.types())
			.unwrap_or(&[])
		{
			let type_id = self.alloc_func_type(ty.clone());
			instance.push_type(type_id);
		}

		{
			let imports = module.import_section().map(|is| is.entries()).unwrap_or(&[]);
			if imports.len() != extern_vals.len() {
				return Err(Error::Initialization(format!("extern_vals length is not equal to import section entries")));
			}

			for (import, extern_val) in Iterator::zip(imports.into_iter(), extern_vals.into_iter())
			{
				match (import.external(), extern_val) {
					(&External::Function(fn_type_idx), &ExternVal::Func(ref func)) => {
						let expected_fn_type = instance.type_by_index(fn_type_idx).expect("Due to validation function type should exists");
						let actual_fn_type = func.func_type();
						if expected_fn_type != actual_fn_type {
							return Err(Error::Initialization(format!(
								"Expected function with type {:?}, but actual type is {:?} for entry {}",
								expected_fn_type,
								actual_fn_type,
								import.field(),
							)));
						}
						instance.push_func(Rc::clone(func))
					}
					(&External::Table(ref tt), &ExternVal::Table(ref table)) => {
						match_limits(table.limits(), tt.limits())?;
						instance.push_table(Rc::clone(table));
					}
					(&External::Memory(ref mt), &ExternVal::Memory(ref memory)) => {
						match_limits(memory.limits(), mt.limits())?;
						instance.push_memory(Rc::clone(memory));
					}
					(&External::Global(ref gl), &ExternVal::Global(ref global)) => {
						// TODO: check globals
						instance.push_global(Rc::clone(global))
					}
					(expected_import, actual_extern_val) => {
						return Err(Error::Initialization(format!(
							"Expected {:?} type, but provided {:?} extern_val",
							expected_import,
							actual_extern_val
						)));
					}
				}
			}
		}

		{
			let funcs = module
				.function_section()
				.map(|fs| fs.entries())
				.unwrap_or(&[]);
			let bodies = module.code_section().map(|cs| cs.bodies()).unwrap_or(&[]);
			debug_assert!(
				funcs.len() == bodies.len(),
				"Due to validation func and body counts must match"
			);

			for (index, (ty, body)) in
				Iterator::zip(funcs.into_iter(), bodies.into_iter()).enumerate()
			{
				let func_type = instance.type_by_index(ty.type_ref()).expect("Due to validation type should exists");
				let labels = aux_data.labels.remove(&index).expect(
					"At func validation time labels are collected; Collected labels are added by index; qed",
				);
				let func_body = FuncBody {
					locals: body.locals().to_vec(),
					opcodes: body.code().clone(),
					labels: labels,
				};
				let func_instance = self.alloc_func(instance, func_type, func_body);
				instance.push_func(func_instance);
			}
		}

		for table_type in module.table_section().map(|ts| ts.entries()).unwrap_or(&[]) {
			let table = self.alloc_table(table_type)?;
			instance.push_table(table);
		}

		for memory_type in module
			.memory_section()
			.map(|ms| ms.entries())
			.unwrap_or(&[])
		{
			let memory = self.alloc_memory(memory_type)?;
			instance.push_memory(memory);
		}

		for global_entry in module
			.global_section()
			.map(|gs| gs.entries())
			.unwrap_or(&[])
		{
			let init_val = eval_init_expr(global_entry.init_expr(), &*instance);
			let global = self.alloc_global(global_entry.global_type().clone(), init_val);
			instance.push_global(global);
		}

		for export in module
			.export_section()
			.map(|es| es.entries())
			.unwrap_or(&[])
		{
			let field = export.field();
			let extern_val: ExternVal = match *export.internal() {
				Internal::Function(idx) => {
					let func = instance
						.func_by_index(idx)
						.expect("Due to validation func should exists");
					ExternVal::Func(func)
				}
				Internal::Global(idx) => {
					let global = instance
						.global_by_index(idx)
						.expect("Due to validation global should exists");
					ExternVal::Global(global)
				}
				Internal::Memory(idx) => {
					let memory = instance
						.memory_by_index(idx)
						.expect("Due to validation memory should exists");
					ExternVal::Memory(memory)
				}
				Internal::Table(idx) => {
					let table = instance
						.table_by_index(idx)
						.expect("Due to validation table should exists");
					ExternVal::Table(table)
				}
			};
			instance.insert_export(field, extern_val);
		}

		Ok(())
	}

	pub fn instantiate_module<St: 'static>(
		&mut self,
		module: &Module,
		extern_vals: &[ExternVal],
		state: &mut St,
	) -> Result<Rc<ModuleInstance>, Error> {
		let mut instance = Rc::new(ModuleInstance::new());

		self.alloc_module_internal(module, extern_vals, &instance)?;

		for element_segment in module
			.elements_section()
			.map(|es| es.entries())
			.unwrap_or(&[])
		{
			let offset_val = match eval_init_expr(element_segment.offset(), &instance) {
				RuntimeValue::I32(v) => v as u32,
				_ => panic!("Due to validation elem segment offset should evaluate to i32"),
			};

			let table_inst = instance
				.table_by_index(DEFAULT_TABLE_INDEX)
				.expect("Due to validation default table should exists");
			for (j, func_idx) in element_segment.members().into_iter().enumerate() {
				let func = instance
					.func_by_index(*func_idx)
					.expect("Due to validation funcs from element segments should exists");

				table_inst.set(offset_val + j as u32, func);
			}
		}

		for data_segment in module.data_section().map(|ds| ds.entries()).unwrap_or(&[]) {
			let offset_val = match eval_init_expr(data_segment.offset(), &instance) {
				RuntimeValue::I32(v) => v as u32,
				_ => panic!("Due to validation data segment offset should evaluate to i32"),
			};

			let memory_inst = instance
				.memory_by_index(DEFAULT_MEMORY_INDEX)
				.expect("Due to validation default memory should exists");
			memory_inst.set(offset_val, data_segment.value())?;
		}

		// And run module's start function, if any
		if let Some(start_fn_idx) = module.start_section() {
			let start_func = {
				instance
					.func_by_index(start_fn_idx)
					.expect("Due to validation start function should exists")
			};
			self.invoke(start_func, vec![], state)?;
		}

		Ok(instance)
	}

	pub fn invoke<St: 'static>(
		&mut self,
		func: Rc<FuncInstance>,
		args: Vec<RuntimeValue>,
		state: &mut St,
	) -> Result<Option<RuntimeValue>, Error> {
		enum InvokeKind {
			Internal(FunctionContext),
			Host(Rc<AnyFunc>, Vec<RuntimeValue>),
		}

		let result = match *func {
			FuncInstance::Internal { ref func_type, .. } => {
				let mut args = StackWithLimit::with_data(args, DEFAULT_VALUE_STACK_LIMIT);
				let args = prepare_function_args(func_type, &mut args)?;
				let context = FunctionContext::new(
					Rc::clone(&func),
					DEFAULT_VALUE_STACK_LIMIT,
					DEFAULT_FRAME_STACK_LIMIT,
					func_type,
					args,
				);
				InvokeKind::Internal(context)
			}
			FuncInstance::Host { ref host_func, .. } => InvokeKind::Host(Rc::clone(host_func), args),
		};

		match result {
			InvokeKind::Internal(ctx) => {
				let mut interpreter = Interpreter::new(self, state);
				interpreter.run_function(ctx)
			}
			InvokeKind::Host(host_func, args) => {
				host_func.call_as_any(self, state as &mut Any, &args)
			}
		}
	}
}

fn eval_init_expr(init_expr: &InitExpr, module: &ModuleInstance) -> RuntimeValue {
	let code = init_expr.code();
	debug_assert!(
		code.len() == 2,
		"Due to validation `code`.len() should be 2"
	);
	match code[0] {
		Opcode::I32Const(v) => v.into(),
		Opcode::I64Const(v) => v.into(),
		Opcode::F32Const(v) => RuntimeValue::decode_f32(v),
		Opcode::F64Const(v) => RuntimeValue::decode_f64(v),
		Opcode::GetGlobal(idx) => {
			let global = module
				.global_by_index(idx)
				.expect("Due to validation global should exists in module");
			global.get()
		}
		_ => panic!("Due to validation init should be a const expr"),
	}
}

fn match_limits(l1: &ResizableLimits, l2: &ResizableLimits) -> Result<(), Error> {
	if l1.initial() < l2.initial() {
		return Err(Error::Initialization(format!(
			"trying to import with limits l1.initial={} and l2.initial={}",
			l1.initial(),
			l2.initial()
		)));
	}

	match (l1.maximum(), l2.maximum()) {
		(_, None) => (),
		(Some(m1), Some(m2)) if m1 <= m2 => (),
		_ => {
			return Err(Error::Initialization(format!(
				"trying to import with limits l1.max={:?} and l2.max={:?}",
				l1.maximum(),
				l2.maximum()
			)))
		}
	}

	Ok(())
}
