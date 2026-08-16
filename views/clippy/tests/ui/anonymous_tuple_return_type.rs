#![allow(dead_code)]
#![warn(clippy::anonymous_tuple_return_type)]

struct Dimensions {
    width: u32,
    height: u32,
}

struct TupleStruct(u32, u32);

type DimensionPair = (u32, u32);

fn direct_tuple() -> (u32, String) {
    //~^ anonymous_tuple_return_type
    (1, String::new())
}

fn single_element_tuple() -> (u32,) {
    //~^ anonymous_tuple_return_type
    (1,)
}

fn nested_tuple() -> Result<(u32, String), String> {
    //~^ anonymous_tuple_return_type
    Ok((1, String::new()))
}

fn named_struct() -> Dimensions {
    Dimensions { width: 1, height: 2 }
}

fn tuple_struct() -> TupleStruct {
    TupleStruct(1, 2)
}

fn type_alias() -> DimensionPair {
    (1, 2)
}

async fn async_tuple() -> (u32, String) {
    //~^ anonymous_tuple_return_type
    (1, String::new())
}

fn function_pointer_return() -> fn() -> (u32, String) {
    //~^ anonymous_tuple_return_type
    direct_tuple
}

trait Trait {
    fn required() -> (u32, String);
    //~^ anonymous_tuple_return_type

    fn provided() -> Option<(u32, String)> {
        //~^ anonymous_tuple_return_type
        Some((1, String::new()))
    }
}

struct Impl;

impl Impl {
    fn method() -> (u32, String) {
        //~^ anonymous_tuple_return_type
        (1, String::new())
    }
}

impl Trait for Impl {
    fn required() -> (u32, String) {
        (1, String::new())
    }
}

fn main() {
    let _closure = || -> (u32, String) {
        //~^ anonymous_tuple_return_type
        (1, String::new())
    };
}

// Parenthesised `Fn`-trait sugar lowers the callable's argument list into a
// tuple (`FnOnce(A, B)` is `FnOnce<(A, B)>`). That tuple is a parameter list,
// not a returned value, so none of these may fire.
fn fn_trait_sugar_one_argument() -> impl FnOnce(String) -> String {
    |suffix| suffix
}

fn fn_trait_sugar_two_arguments() -> impl FnMut(u32, u32) -> u32 {
    |a, b| a + b
}

fn dyn_fn_trait_sugar_argument() -> Box<dyn Fn(u32, String) -> u32> {
    Box::new(|a, _b| a)
}

fn fn_pointer_tuple_argument() -> fn((u32, u32)) -> u32 {
    |pair| pair.0
}

fn tuple_in_argument_position(make: impl FnOnce() -> (u32, String)) -> u32 {
    make().0
}

fn tuple_in_where_bound<F>(make: F) -> u32
where
    F: FnOnce() -> (u32, String),
{
    make().0
}

trait SugarTrait {
    fn required_sugar_argument() -> Box<dyn FnOnce(u32, u32) -> u32>;
}

// The sugar's `Output` is a return position, so an anonymous tuple there still
// fires, and the span must point at the output rather than the argument list.
fn fn_trait_sugar_output() -> impl Fn() -> (u32, String) {
    //~^ anonymous_tuple_return_type
    || (1, String::new())
}

fn dyn_fn_trait_sugar_output() -> Box<dyn Fn(u32) -> (u32, String)> {
    //~^ anonymous_tuple_return_type
    Box::new(|a| (a, String::new()))
}

trait SugarOutputTrait {
    fn required_sugar_output() -> Box<dyn FnOnce() -> (u32, String)>;
    //~^ anonymous_tuple_return_type
}
