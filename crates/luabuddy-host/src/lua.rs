use jni::{
    JNIEnv,
    objects::{GlobalRef, JObject, JObjectArray, JString, JValue},
};
use std::sync::LazyLock;

static NIL: LazyLock<JObject<'static>> = LazyLock::new(JObject::null);

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Default)]
pub struct Value(pub Option<GlobalRef>);

impl Value {
    pub fn is_nil(&self) -> bool {
        self.0.is_none()
    }
    pub fn object(&self) -> &JObject<'static> {
        match &self.0 {
            Some(reference) => reference.as_obj(),
            None => &NIL,
        }
    }
}

pub struct Context<'a, 'vm> {
    pub env: &'a mut JNIEnv<'vm>,
    pub environment: Value,
    thread: Value,
    caller: Value,
}

impl<'a, 'vm> Context<'a, 'vm> {
    pub fn current(env: &'a mut JNIEnv<'vm>) -> Result<Self> {
        let environment = Self::static_value(env, "env", "Lse/krka/kahlua/vm/KahluaTable;")?;
        let thread = Self::static_value(env, "thread", "Lse/krka/kahlua/vm/KahluaThread;")?;
        let caller = Self::static_value(env, "caller", "Lse/krka/kahlua/integration/LuaCaller;")?;
        if environment.is_nil() || thread.is_nil() || caller.is_nil() {
            return Err("The game's Lua environment is unavailable".into());
        }
        Ok(Self {
            env,
            environment,
            thread,
            caller,
        })
    }

    fn static_value(env: &mut JNIEnv<'vm>, name: &str, signature: &str) -> Result<Value> {
        let object = env
            .get_static_field("zombie/Lua/LuaManager", name, signature)
            .and_then(|v| v.l())
            .map_err(|e| e.to_string())?;
        Self::retain(env, &object)
    }

    pub fn retain(env: &mut JNIEnv<'vm>, object: &JObject<'_>) -> Result<Value> {
        if object.is_null() {
            Ok(Value::default())
        } else {
            env.new_global_ref(object)
                .map(|v| Value(Some(v)))
                .map_err(|e| e.to_string())
        }
    }

    pub fn jni<T>(
        &mut self,
        action: impl FnOnce(&mut JNIEnv<'vm>) -> jni::errors::Result<T>,
    ) -> Result<T> {
        match action(self.env) {
            Ok(value) => Ok(value),
            Err(error) => {
                let mut detail = error.to_string();
                if self.env.exception_check().unwrap_or(false)
                    && let Ok(cause) = self.env.exception_occurred()
                {
                    let _ = self.env.exception_clear();
                    if let Ok(text) = self
                        .env
                        .call_method(&cause, "toString", "()Ljava/lang/String;", &[])
                        .and_then(|v| v.l())
                        && let Ok(text) = self.env.get_string(&JString::from(text))
                    {
                        detail.push_str(&format!("\nJava cause: {}", text.to_string_lossy()));
                    }
                    let _ = self.env.exception_clear();
                }
                Err(detail)
            }
        }
    }

    pub fn string(&mut self, text: &str) -> Result<Value> {
        self.jni(|env| {
            let value = env.new_string(text)?;
            env.new_global_ref(value).map(|v| Value(Some(v)))
        })
    }

    pub fn boolean(&mut self, value: bool) -> Result<Value> {
        self.jni(|env| {
            let value = env
                .call_static_method(
                    "java/lang/Boolean",
                    "valueOf",
                    "(Z)Ljava/lang/Boolean;",
                    &[JValue::Bool(value.into())],
                )?
                .l()?;
            env.new_global_ref(value).map(|v| Value(Some(v)))
        })
    }

    pub fn number(&mut self, value: f64) -> Result<Value> {
        self.jni(|env| {
            let value = env
                .call_static_method(
                    "java/lang/Double",
                    "valueOf",
                    "(D)Ljava/lang/Double;",
                    &[JValue::Double(value)],
                )?
                .l()?;
            env.new_global_ref(value).map(|v| Value(Some(v)))
        })
    }

    pub fn table(&mut self) -> Result<Value> {
        self.jni(|env| {
            let map = env.new_object("java/util/LinkedHashMap", "()V", &[])?;
            let value = env.new_object(
                "se/krka/kahlua/j2se/KahluaTableImpl",
                "(Ljava/util/Map;)V",
                &[JValue::Object(&map)],
            )?;
            env.new_global_ref(value).map(|v| Value(Some(v)))
        })
    }

    pub fn text(&mut self, value: &Value) -> Result<String> {
        if value.is_nil() {
            return Ok("nil".into());
        }
        self.jni(|env| {
            let object = env
                .call_method(value.object(), "toString", "()Ljava/lang/String;", &[])?
                .l()?;
            let text = env
                .get_string(&JString::from(object))?
                .to_string_lossy()
                .into_owned();
            Ok(text)
        })
    }

    pub fn truth(&mut self, value: &Value) -> Result<bool> {
        if value.is_nil() {
            return Ok(false);
        }
        self.jni(|env| {
            if env.is_instance_of(value.object(), "java/lang/Boolean")? {
                env.call_method(value.object(), "booleanValue", "()Z", &[])?
                    .z()
            } else {
                Ok(true)
            }
        })
    }

    pub fn numeric(&mut self, value: &Value) -> Result<f64> {
        self.jni(|env| {
            env.call_method(value.object(), "doubleValue", "()D", &[])?
                .d()
        })
    }

    pub fn java_trace(&mut self, cause: &Value) -> Result<String> {
        if cause.is_nil() {
            return Ok(String::new());
        }
        if !self.jni(|env| env.is_instance_of(cause.object(), "java/lang/Throwable"))? {
            return self.text(cause);
        }
        let text = self.jni(|env| {
            let writer = env.new_object("java/io/StringWriter", "()V", &[])?;
            let print = env.new_object(
                "java/io/PrintWriter",
                "(Ljava/io/Writer;)V",
                &[JValue::Object(&writer)],
            )?;
            env.call_method(
                cause.object(),
                "printStackTrace",
                "(Ljava/io/PrintWriter;)V",
                &[JValue::Object(&print)],
            )?;
            let text = env
                .call_method(writer, "toString", "()Ljava/lang/String;", &[])?
                .l()?;
            env.new_global_ref(text).map(|value| Value(Some(value)))
        })?;
        self.text(&text)
    }

    pub fn same(&mut self, left: &Value, right: &Value) -> Result<bool> {
        self.jni(|env| env.is_same_object(left.object(), right.object()))
    }

    pub fn global(&mut self, name: &str) -> Result<Value> {
        let table = self.environment.clone();
        self.get(&table, name)
    }

    pub fn get(&mut self, table: &Value, name: &str) -> Result<Value> {
        self.jni(|env| {
            let key = env.new_string(name)?;
            let value = env
                .call_method(
                    table.object(),
                    "rawget",
                    "(Ljava/lang/Object;)Ljava/lang/Object;",
                    &[JValue::Object(&key)],
                )?
                .l()?;
            if value.is_null() {
                Ok(Value::default())
            } else {
                env.new_global_ref(value).map(|v| Value(Some(v)))
            }
        })
    }

    pub fn index(&mut self, table: &Value, name: &str) -> Result<Value> {
        let thread = self.thread.clone();
        self.jni(|env| {
            let key = env.new_string(name)?;
            let value = env
                .call_method(
                    thread.object(),
                    "tableget",
                    "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;",
                    &[JValue::Object(table.object()), JValue::Object(&key)],
                )?
                .l()?;
            if value.is_null() {
                Ok(Value::default())
            } else {
                env.new_global_ref(value).map(|v| Value(Some(v)))
            }
        })
    }

    pub fn set(&mut self, table: &Value, name: &str, value: &Value) -> Result<()> {
        self.jni(|env| {
            let key = env.new_string(name)?;
            env.call_method(
                table.object(),
                "rawset",
                "(Ljava/lang/Object;Ljava/lang/Object;)V",
                &[JValue::Object(&key), JValue::Object(value.object())],
            )?;
            Ok(())
        })
    }

    pub fn entries(&mut self, table: &Value) -> Result<Vec<(Value, Value)>> {
        self.jni(|env| {
            let iterator = env
                .call_method(
                    table.object(),
                    "iterator",
                    "()Lse/krka/kahlua/vm/KahluaTableIterator;",
                    &[],
                )?
                .l()?;
            let mut result = Vec::new();
            while env.call_method(&iterator, "advance", "()Z", &[])?.z()? {
                let key = env
                    .call_method(&iterator, "getKey", "()Ljava/lang/Object;", &[])?
                    .l()?;
                let value = env
                    .call_method(&iterator, "getValue", "()Ljava/lang/Object;", &[])?
                    .l()?;
                let key = env.new_global_ref(key)?;
                let value = if value.is_null() {
                    Value::default()
                } else {
                    Value(Some(env.new_global_ref(value)?))
                };
                result.push((Value(Some(key)), value));
            }
            Ok(result)
        })
    }

    pub fn call(&mut self, callable: &Value, arguments: &[Value]) -> Result<Vec<Value>> {
        if callable.is_nil() {
            return Err("Attempted to call a missing host function".into());
        }
        let caller = self.caller.clone();
        let thread = self.thread.clone();
        let result = self.jni(|env| {
            let args = env.new_object_array(arguments.len() as i32, "java/lang/Object", JObject::null())?;
            for (i, arg) in arguments.iter().enumerate() { env.set_object_array_element(&args, i as i32, arg.object())?; }
            let array = env.call_method(caller.object(), "pcall", "(Lse/krka/kahlua/vm/KahluaThread;Ljava/lang/Object;[Ljava/lang/Object;)[Ljava/lang/Object;", &[JValue::Object(thread.object()), JValue::Object(callable.object()), JValue::Object(&args)])?.l()?;
            let array = JObjectArray::from(array);
            let mut values = Vec::new();
            for i in 0..env.get_array_length(&array)? { let value = env.get_object_array_element(&array, i)?; values.push(if value.is_null() { Value::default() } else { Value(Some(env.new_global_ref(value)?)) }); }
            Ok(values)
        })?;
        if result.is_empty() || !self.truth(&result[0])? {
            let mut message = "Host Lua call failed".to_string();
            if let Some(error) = result.get(1) {
                message.push_str(&format!(": {}", self.text(error)?));
            }
            if let Some(trace) = result.get(2).filter(|trace| !trace.is_nil()) {
                message.push_str(&format!("\nLua traceback:\n{}", self.text(trace)?));
            }
            if let Some(cause) = result.get(3).filter(|cause| !cause.is_nil()) {
                message.push_str(&format!("\nJava traceback:\n{}", self.java_trace(cause)?));
            }
            return Err(message);
        }
        Ok(result.into_iter().skip(1).collect())
    }

    pub fn invoke(&mut self, receiver: &Value, name: &str, args: &[Value]) -> Result<Vec<Value>> {
        let function = self.index(receiver, name)?;
        let mut all = Vec::with_capacity(args.len() + 1);
        all.push(receiver.clone());
        all.extend_from_slice(args);
        self.call(&function, &all)
    }

    pub fn call_global(&mut self, name: &str, args: &[Value]) -> Result<Vec<Value>> {
        let function = self.global(name)?;
        self.call(&function, args)
    }

    pub fn compile(&mut self, text: &str, filename: &str) -> Result<Value> {
        let environment = self.environment.clone();
        self.jni(|env| {
            let source = env.new_string(text)?; let filename = env.new_string(filename)?;
            let closure = env.call_static_method("se/krka/kahlua/luaj/compiler/LuaCompiler", "loadstring", "(Ljava/lang/String;Ljava/lang/String;Lse/krka/kahlua/vm/KahluaTable;)Lse/krka/kahlua/vm/LuaClosure;", &[JValue::Object(&source), JValue::Object(&filename), JValue::Object(environment.object())])?.l()?;
            let prototype = env.get_field(&closure, "prototype", "Lse/krka/kahlua/vm/Prototype;")?.l()?;
            source_metadata(env, &prototype, &filename)?;
            env.new_global_ref(closure).map(|v| Value(Some(v)))
        })
    }

    pub fn frame_arguments(&mut self, frame: &JObject<'_>, count: i32) -> Result<Vec<Value>> {
        if !(0..=1024).contains(&count) {
            return Err("Invalid native Lua argument count".into());
        }
        self.jni(|env| {
            let mut args = Vec::new();
            for i in 0..count {
                let arg = env
                    .call_method(frame, "get", "(I)Ljava/lang/Object;", &[JValue::Int(i)])?
                    .l()?;
                args.push(if arg.is_null() {
                    Value::default()
                } else {
                    Value(Some(env.new_global_ref(arg)?))
                });
            }
            Ok(args)
        })
    }

    pub fn frame_return(&mut self, frame: &JObject<'_>, values: &[Value]) -> Result<i32> {
        for value in values {
            self.jni(|env| {
                env.call_method(
                    frame,
                    "push",
                    "(Ljava/lang/Object;)I",
                    &[JValue::Object(value.object())],
                )
            })?;
        }
        Ok(values.len() as i32)
    }
}

fn source_metadata(
    env: &mut JNIEnv<'_>,
    prototype: &JObject<'_>,
    filename: &JString<'_>,
) -> jni::errors::Result<()> {
    env.set_field(
        prototype,
        "filename",
        "Ljava/lang/String;",
        JValue::Object(filename),
    )?;
    let path = env.get_string(filename)?.to_string_lossy().into_owned();
    let file = env.new_string(path.rsplit('/').next().unwrap_or(&path))?;
    env.set_field(
        prototype,
        "file",
        "Ljava/lang/String;",
        JValue::Object(&file),
    )?;
    if env
        .get_field(prototype, "name", "Ljava/lang/String;")?
        .l()?
        .is_null()
    {
        env.set_field(
            prototype,
            "name",
            "Ljava/lang/String;",
            JValue::Object(filename),
        )?;
    }
    let children = env
        .get_field(prototype, "prototypes", "[Lse/krka/kahlua/vm/Prototype;")?
        .l()?;
    if !children.is_null() {
        let children = JObjectArray::from(children);
        for index in 0..env.get_array_length(&children)? {
            let child = env.get_object_array_element(&children, index)?;
            let child = env.auto_local(child);
            source_metadata(env, &child, filename)?;
        }
    }
    Ok(())
}
