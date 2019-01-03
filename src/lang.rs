use crate::shadowenv::Shadowenv;
use crate::hash::Source;

use std::rc::Rc;
use ketos::{Error, Interpreter, Value, FromValueRef};
use std::path::PathBuf;

pub struct ShadowLang {}

macro_rules! ketos_fn2 {
    ( $scope:expr => $name:expr => fn $ident:ident
            (...) -> $res:ty ) => {
        $scope.add_value_with_name($name,
            |name| Value::new_foreign_fn(name, move |_scope, args| {
                let res = $ident(args)?;
                Ok(<$res as Into<Value>>::into(res))
            }))
    }
}

fn path_concat(vals: &mut [Value]) -> Result<String, Error> {
    let res = vals.iter().fold(PathBuf::new(), |acc, v|
        acc.join(<&str as FromValueRef>::from_value_ref(v).unwrap()) // TODO(burke): don't unwrap
    );

    Ok(res.to_string_lossy().to_string())
}

macro_rules! assert_args {
    ( $args:expr , $count:expr , $name:expr ) => {
        if $args.len() != $count {
            return Err(From::from(ketos::exec::ExecError::ArityError{
                name: Some($name),
                expected: ketos::function::Arity::Exact($count as u32),
                found: $args.len() as u32,
            }));
        }
    }
}

// macro_rules! sekfn {
//     ( $scope:expr => $shadowenv_name:expr => $name:expr => fn $ident:ident
//             ( $( $arg:ident : $arg_ty:ty ),* ) -> res { $body:expr } ) => {
//         $scope.add_value_with_name($name,
//             |name| ketos::value::Value::new_foreign_fn(name, move |ctx, args| {
//                 let expected = 0 $( + { stringify!($arg); 1 } )*;

//                 if args.len() != expected {
//                     return Err(From::from(ketos::exec::ExecError::ArityError{
//                         name: Some(name),
//                         expected: ketos::function::Arity::Exact(expected as u32),
//                         found: args.len() as u32,
//                     }));
//                 }

//                 let mut iter = (&*args).iter();

//                 $(
//                     let $arg = (<$arg_ty as ketos::value::FromValueRef>::from_value_ref(iter.next().unwrap()))?;
//                 ),*

//                 let value = ctx.scope().get_constant($shadowenv_name).expect("bug: shadowenv not defined");
//                 let __shadowenv = <&Shadowenv as FromValueRef>::from_value_ref(&value)?;

//                 let body = $body;
//                 Ok(ketos::value::Value::from(body))
//             }))
//     }
// }

impl ShadowLang {
    pub fn run_program(shadowenv: Rc<Shadowenv>, source: Source) -> Result<(), Error> {
        let interp = Interpreter::new();

        let shadowenv_name = interp.scope().add_name("shadowenv");
        interp.scope().add_constant(shadowenv_name, Value::Foreign(shadowenv.clone()));

        ketos_fn2!{ interp.scope() => "path-concat" =>
            fn path_concat(...) -> String }

        // sekfn!{ interp.scope() => shadowenv_name => "env/get" => fn env_get(name: &str) -> res {
        //     __shadowenv.env_get(name)
        //         .map(|s| <String as Into<Value>>::into(s.to_string()))
        //         .unwrap_or(Value::Unit)
        // }}

        interp.scope().add_value_with_name("env/get", |name| Value::new_foreign_fn(name, move |ctx, args| {
            assert_args!(args, 1, name);

            let value = ctx.scope().get_constant(shadowenv_name).expect("bug: shadowenv not defined");
            let shadowenv = <&Shadowenv as FromValueRef>::from_value_ref(&value)?;
            let name = <&str as FromValueRef>::from_value_ref(&args[0])?;

            let foo = shadowenv.get(name)
                .map(|s| <String as Into<Value>>::into(s.to_string()))
                .unwrap_or(Value::Unit);
            Ok(foo)
        }));

        interp.scope().add_value_with_name("env/set", |name| Value::new_foreign_fn(name, move |ctx, args| {
            assert_args!(args, 2, name);

            let value = ctx.scope().get_constant(shadowenv_name).expect("bug: shadowenv not defined");
            let shadowenv = <&Shadowenv as FromValueRef>::from_value_ref(&value)?;
            let name = <&str as FromValueRef>::from_value_ref(&args[0])?;
            let value = <&str as FromValueRef>::from_value_ref(&args[1]).ok();

            shadowenv.set(name, value);
            Ok(Value::Unit)
        }));

        interp.scope().add_value_with_name("env/prepend-to-pathlist", |name| Value::new_foreign_fn(name, move |ctx, args| {
            assert_args!(args, 2, name);

            let value = ctx.scope().get_constant(shadowenv_name).expect("bug: shadowenv not defined");
            let shadowenv = <&Shadowenv as FromValueRef>::from_value_ref(&value)?;
            let name  = <&str as FromValueRef>::from_value_ref(&args[0])?;
            let value = <&str as FromValueRef>::from_value_ref(&args[1])?;

            shadowenv.prepend_to_pathlist(name, value);
            Ok(Value::Unit)
        }));

        interp.scope().add_value_with_name("env/remove-from-pathlist", |name| Value::new_foreign_fn(name, move |ctx, args| {
            assert_args!(args, 2, name);

            let value = ctx.scope().get_constant(shadowenv_name).expect("bug: shadowenv not defined");
            let shadowenv = <&Shadowenv as FromValueRef>::from_value_ref(&value)?;
            let name  = <&str as FromValueRef>::from_value_ref(&args[0])?;
            let value = <&str as FromValueRef>::from_value_ref(&args[1])?;

            shadowenv.remove_from_pathlist(name, value);
            Ok(Value::Unit)
        }));

        // TODO(burke): expand-path isn't even implemented
        let prelude = r#"
          ;; Path manipulation stuff
          (define (expand-path path) path)

          ;; Better when/if/let macros
          (macro (when pred :rest body) `(if ,pred (do ,@body) ()))
          (macro (when-let assigns :rest body)
            `(let ,assigns (when (not (null ,(first (first assigns)))) ,@body)))
        "#;

        if let Err(err) = interp.run_code(&prelude, None) {
            interp.display_error(&err);
            if let Some(trace) = interp.get_traceback() {
                eprintln!("");
                interp.display_trace(&trace);
            }
            panic!();
        };

        for source_file in &source.files {
            let fname = format!("__shadowenv__{}", source_file.name);
            let prog = format!("(define ({} env) (do {}))", fname, source_file.source);

            if let Err(err) = interp.run_code(&prog, Some(source_file.name.to_string())) {
                interp.display_error(&err);
                if let Some(trace) = interp.get_traceback() {
                    eprintln!("");
                    interp.display_trace(&trace);
                }
                return Ok(());
            };
        }

        for source_file in source.files {
            let fname = format!("__shadowenv__{}", source_file.name);
            if let Err(err) = interp.call(&fname, vec![Value::Foreign(shadowenv.clone())]) {
                interp.display_error(&err);
                if let Some(trace) = interp.get_traceback() {
                    eprintln!("");
                    interp.display_trace(&trace);
                }
                return Ok(());
            };
        }

        Ok(())
    }
}