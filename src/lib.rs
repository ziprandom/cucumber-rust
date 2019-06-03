// Copyright (c) 2018  Brendan Molloy <brendan@bbqsrc.net>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

pub extern crate gherkin_rust as gherkin;
pub extern crate globwalk;

pub mod cli;
mod hashable_regex;
mod output;
mod panic_trap;

use std::collections::HashMap;
use std::fs::File;
use std::io::{stderr, Read, Write};
use std::path::PathBuf;

use gherkin::Feature;
pub use gherkin::{Scenario, Step, StepType};
use regex::Regex;

use hashable_regex::HashableRegex;
pub use output::default::DefaultOutput;
use output::OutputVisitor;
use panic_trap::{PanicDetails, PanicTrap};

pub trait World: Default {}

type HelperFn = fn(&Scenario) -> ();

type TestFn<T> = fn(&mut T, &Step) -> ();
type TestRegexFn<T> = fn(&mut T, &[String], &Step) -> ();

struct TestCase<T: Default>(pub TestFn<T>);
struct RegexTestCase<T: Default>(pub TestRegexFn<T>);

type TestBag<T> = HashMap<&'static str, TestCase<T>>;
type RegexBag<T> = HashMap<HashableRegex, RegexTestCase<T>>;

#[derive(Default)]
pub struct Steps<T: Default> {
    given: TestBag<T>,
    when: TestBag<T>,
    then: TestBag<T>,
    regex: RegexSteps<T>,
}

#[derive(Default)]
struct RegexSteps<T: Default> {
    given: RegexBag<T>,
    when: RegexBag<T>,
    then: RegexBag<T>,
}

enum TestCaseType<'a, T: 'a + Default> {
    Normal(&'a TestCase<T>),
    Regex(&'a RegexTestCase<T>, Vec<String>),
}

pub enum TestResult {
    MutexPoisoned,
    Skipped,
    Unimplemented,
    Pass,
    Fail(PanicDetails, Vec<u8>, Vec<u8>),
}

impl<T: Default> Steps<T> {
    fn test_bag_for(&self, ty: StepType) -> &TestBag<T> {
        match ty {
            StepType::Given => &self.given,
            StepType::When => &self.when,
            StepType::Then => &self.then,
        }
    }

    fn test_bag_mut_for(&mut self, ty: StepType) -> &mut TestBag<T> {
        match ty {
            StepType::Given => &mut self.given,
            StepType::When => &mut self.when,
            StepType::Then => &mut self.then,
        }
    }

    fn regex_bag_for(&self, ty: StepType) -> &RegexBag<T> {
        match ty {
            StepType::Given => &self.regex.given,
            StepType::When => &self.regex.when,
            StepType::Then => &self.regex.then,
        }
    }

    fn regex_bag_mut_for(&mut self, ty: StepType) -> &mut RegexBag<T> {
        match ty {
            StepType::Given => &mut self.regex.given,
            StepType::When => &mut self.regex.when,
            StepType::Then => &mut self.regex.then,
        }
    }

    fn test_type<'a>(&'a self, step: &Step) -> Option<TestCaseType<'a, T>> {
        if let Some(t) = self.test_bag_for(step.ty).get(&*step.value) {
            return Some(TestCaseType::Normal(t));
        }

        if let Some((regex, t)) = self
            .regex_bag_for(step.ty)
            .iter()
            .find(|(regex, _)| regex.is_match(&step.value))
        {
            let matches = regex
                .0
                .captures(&step.value)
                .unwrap()
                .iter()
                .map(|match_| {
                    match_
                        .map(|match_| match_.as_str().to_owned())
                        .unwrap_or_default()
                })
                .collect();

            return Some(TestCaseType::Regex(t, matches));
        }

        None
    }

    pub fn add_normal(&mut self, ty: StepType, name: &'static str, test_fn: TestFn<T>) {
        self.test_bag_mut_for(ty).insert(name, TestCase(test_fn));
    }

    pub fn add_regex(&mut self, ty: StepType, regex: &str, test_fn: TestRegexFn<T>) {
        let regex = Regex::new(regex)
            .unwrap_or_else(|_| panic!("`{}` is not a valid regular expression", regex));

        self.regex_bag_mut_for(ty)
            .insert(HashableRegex(regex), RegexTestCase(test_fn));
    }

    pub fn combine(iter: impl Iterator<Item = Self>) -> Self {
        let mut combined = Self::default();

        for steps in iter {
            combined.given.extend(steps.given);
            combined.when.extend(steps.when);
            combined.then.extend(steps.then);

            combined.regex.given.extend(steps.regex.given);
            combined.regex.when.extend(steps.regex.when);
            combined.regex.then.extend(steps.regex.then);
        }

        combined
    }

    fn run_test(&self, world: &mut T, step: &Step, suppress_output: bool) -> Option<TestResult> {
        let test_type = self.test_type(step)?;

        let test_result = PanicTrap::run(suppress_output, move || match test_type {
            TestCaseType::Normal(t) => (t.0)(world, &step),
            TestCaseType::Regex(t, ref c) => (t.0)(world, c, &step),
        });

        Some(match test_result.result {
            Ok(_) => TestResult::Pass,
            Err(panic_info) => {
                if panic_info.payload.ends_with("cucumber test skipped") {
                    TestResult::Skipped
                } else {
                    TestResult::Fail(panic_info, test_result.stdout, test_result.stderr)
                }
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn run_scenario(
        &self,
        feature: &gherkin::Feature,
        rule: Option<&gherkin::Rule>,
        scenario: &gherkin::Scenario,
        example: Option<&[String]>,
        before_fns: &Option<&[HelperFn]>,
        after_fns: &Option<&[HelperFn]>,
        suppress_output: bool,
        output: &mut impl OutputVisitor,
    ) -> bool {
        output.visit_scenario(rule, &scenario);

        if let Some(before_fns) = before_fns {
            for f in before_fns.iter() {
                f(&scenario);
            }
        }

        let mut world = {
            let panic_trap = PanicTrap::run(suppress_output, T::default);
            match panic_trap.result {
                Ok(v) => v,
                Err(panic_info) => {
                    eprintln!(
                        "Panic caught during world creation. Panic location: {}",
                        panic_info.location
                    );
                    if !panic_trap.stdout.is_empty() {
                        eprintln!("Captured output was:");
                        Write::write(&mut stderr(), &panic_trap.stdout).unwrap();
                    }
                    panic!(panic_info.payload);
                }
            }
        };

        let mut is_success = true;
        let mut is_skipping = false;

        let steps = feature
            .background
            .iter()
            .map(|bg| bg.steps.iter())
            .flatten()
            .chain(scenario.steps.iter());

        for step in steps {
            output.visit_step(rule, &scenario, &step);

            let result = example.map_or(
                self.run_test(&mut world, &step, suppress_output),
                |example| {
                    let interpolated_step = interpolate_outline_variables(
                        step,
                        &scenario.examples.as_ref().unwrap().table.header,
                        example,
                    );
                    self.run_test(&mut world, &interpolated_step, suppress_output)
                },
            );

            let result = match result {
                Some(v) => v,
                None => {
                    output.visit_step_result(rule, &scenario, &step, &TestResult::Unimplemented);
                    if !is_skipping {
                        is_skipping = true;
                        output.visit_scenario_skipped(rule, &scenario);
                    }
                    continue;
                }
            };

            if is_skipping {
                output.visit_step_result(rule, &scenario, &step, &TestResult::Skipped);
            } else {
                output.visit_step_result(rule, &scenario, &step, &result);
                match result {
                    TestResult::Pass => {}
                    TestResult::Fail(_, _, _) => {
                        is_success = false;
                        is_skipping = true;
                    }
                    _ => {
                        is_skipping = true;
                        output.visit_scenario_skipped(rule, &scenario);
                    }
                };
            }
        }

        if let Some(after_fns) = after_fns {
            for f in after_fns.iter() {
                f(&scenario);
            }
        }

        output.visit_scenario_end(rule, &scenario, example);

        is_success
    }

    #[allow(clippy::too_many_arguments)]
    fn run_scenarios(
        &self,
        feature: &gherkin::Feature,
        rule: Option<&gherkin::Rule>,
        scenarios: &[gherkin::Scenario],
        before_fns: Option<&[HelperFn]>,
        after_fns: Option<&[HelperFn]>,
        options: &cli::CliOptions,
        output: &mut impl OutputVisitor,
    ) -> bool {
        let mut is_success = true;

        for scenario in scenarios {
            // If a tag is specified and the scenario does not have the tag, skip the test.
            let should_skip = match (&scenario.tags, &options.tag) {
                (Some(ref tags), Some(ref tag)) => !tags.contains(tag),
                _ => false,
            };

            if should_skip {
                continue;
            }

            // If regex filter fails, skip the test.
            if let Some(ref regex) = options.filter {
                if !regex.is_match(&scenario.name) {
                    continue;
                }
            }

            if let Some(examples) = scenario.examples.as_ref() {
                for example in &examples.table.rows {
                    if !self.run_scenario(
                        &feature,
                        rule,
                        &scenario,
                        Some(&example),
                        &before_fns,
                        &after_fns,
                        options.suppress_output,
                        output,
                    ) {
                        is_success = false;
                    }
                }
            } else if !self.run_scenario(
                &feature,
                rule,
                &scenario,
                None,
                &before_fns,
                &after_fns,
                options.suppress_output,
                output,
            ) {
                is_success = false;
            }
        }

        is_success
    }

    pub fn run(
        &self,
        feature_files: Vec<PathBuf>,
        before_fns: Option<&[HelperFn]>,
        after_fns: Option<&[HelperFn]>,
        options: cli::CliOptions,
        output: &mut impl OutputVisitor,
    ) -> bool {
        output.visit_start();

        let mut is_success = true;

        for path in feature_files {
            let mut file = File::open(&path).expect("file to open");
            let mut buffer = String::new();
            file.read_to_string(&mut buffer).unwrap();

            let feature = match Feature::try_from(&*buffer) {
                Ok(v) => v,
                Err(e) => {
                    output.visit_feature_error(&path, &e);
                    is_success = false;
                    continue;
                }
            };

            output.visit_feature(&feature, &path);
            if !self.run_scenarios(
                &feature,
                None,
                &feature.scenarios,
                before_fns,
                after_fns,
                &options,
                output,
            ) {
                is_success = false;
            }

            for rule in &feature.rules {
                output.visit_rule(&rule);
                if !self.run_scenarios(
                    &feature,
                    Some(&rule),
                    &rule.scenarios,
                    before_fns,
                    after_fns,
                    &options,
                    output,
                ) {
                    is_success = false;
                }
                output.visit_rule_end(&rule);
            }
            output.visit_feature_end(&feature);
        }

        output.visit_finish();

        is_success
    }
}

fn interpolate_outline_variables(
    step: &gherkin::Step,
    header: &[String],
    example: &[String],
) -> gherkin::Step {
    let keys = header
        .iter()
        .map(|key| format!("<{}>", key))
        .collect::<Vec<_>>();

    let replace_vars = |text: &mut String| {
        keys.iter().zip(example).for_each(|(key, value)| {
            *text = text.replace(key, &value);
        });
    };

    let mut step = step.clone();

    // step text
    replace_vars(&mut step.value);

    // step table
    if let Some(ref mut table) = step.table {
        for row in &mut table.rows {
            for cell in row {
                replace_vars(cell);
            }
        }
    }

    // step docstring
    if let Some(docstring) = &mut step.docstring {
        replace_vars(docstring);
    }

    step
}

#[doc(hidden)]
pub fn tag_rule_applies(scenario: &Scenario, rule: &str) -> bool {
    if let Some(ref tags) = &scenario.tags {
        let tags: Vec<&str> = tags.iter().map(|s| s.as_str()).collect();
        let rule_chunks = rule.split(' ');
        // TODO: implement a sane parser for this
        for rule in rule_chunks {
            if rule == "and" || rule == "or" {
                // TODO: implement handling for this
                continue;
            }

            if !tags.contains(&rule) {
                return false;
            }
        }

        true
    } else {
        true
    }
}

#[macro_export]
macro_rules! before {
    (
        $fnname:ident: $tagrule:tt => $scenariofn:expr
    ) => {
        fn $fnname(scenario: &$crate::Scenario) {
            let scenario_closure: fn(&$crate::Scenario) -> () = $scenariofn;
            let tag_rule: &str = $tagrule;

            // TODO check tags
            if $crate::tag_rule_applies(scenario, tag_rule) {
                scenario_closure(scenario);
            }
        }
    };

    (
        $fnname:ident => $scenariofn:expr
    ) => {
        before!($fnname: "" => $scenariofn);
    };
}

// This is just a remap of before.
#[macro_export]
macro_rules! after {
    (
        $fnname:ident: $tagrule:tt => $stepfn:expr
    ) => {
        before!($fnname: $tagrule => $stepfn);
    };

    (
        $fnname:ident => $scenariofn:expr
    ) => {
        before!($fnname: "" => $scenariofn);
    };
}

#[macro_export]
macro_rules! cucumber {
    (
        features: $featurepath:tt,
        world: $worldtype:path,
        steps: $vec:expr,
        setup: $setupfn:expr,
        before: $beforefns:expr,
        after: $afterfns:expr
    ) => {
        cucumber!(@finish; $featurepath; $worldtype; $vec; Some($setupfn); Some($beforefns); Some($afterfns));
    };

    (
        features: $featurepath:tt,
        world: $worldtype:path,
        steps: $vec:expr,
        setup: $setupfn:expr,
        before: $beforefns:expr
    ) => {
        cucumber!(@finish; $featurepath; $worldtype; $vec; Some($setupfn); Some($beforefns); None);
    };

        (
        features: $featurepath:tt,
        world: $worldtype:path,
        steps: $vec:expr,
        setup: $setupfn:expr,
        after: $afterfns:expr
    ) => {
        cucumber!(@finish; $featurepath; $worldtype; $vec; Some($setupfn); None; Some($afterfns));
    };

    (
        features: $featurepath:tt,
        world: $worldtype:path,
        steps: $vec:expr,
        before: $beforefns:expr,
        after: $afterfns:expr
    ) => {
        cucumber!(@finish; $featurepath; $worldtype; $vec; None; Some($beforefns); Some($afterfns));
    };

    (
        features: $featurepath:tt,
        world: $worldtype:path,
        steps: $vec:expr,
        before: $beforefns:expr
    ) => {
        cucumber!(@finish; $featurepath; $worldtype; $vec; None; Some($beforefns); None);
    };

    (
        features: $featurepath:tt,
        world: $worldtype:path,
        steps: $vec:expr,
        after: $afterfns:expr
    ) => {
        cucumber!(@finish; $featurepath; $worldtype; $vec; None; None; Some($afterfns));
    };

    (
        features: $featurepath:tt,
        world: $worldtype:path,
        steps: $vec:expr,
        setup: $setupfn:expr
    ) => {
        cucumber!(@finish; $featurepath; $worldtype; $vec; Some($setupfn); None; None);
    };

    (
        features: $featurepath:tt,
        world: $worldtype:path,
        steps: $vec:expr
    ) => {
        cucumber!(@finish; $featurepath; $worldtype; $vec; None; None; None);
    };

    (
        @finish; $featurepath:tt; $worldtype:path; $vec:expr; $setupfn:expr; $beforefns:expr; $afterfns:expr
    ) => {
        #[allow(unused_imports)]
        fn main() {
            use std::path::Path;
            use std::process;
            use $crate::globwalk::{glob, GlobWalkerBuilder};
            use $crate::gherkin::Scenario;
            use $crate::{Steps, World, DefaultOutput};
            use $crate::cli::make_app;

            let options = match make_app() {
                Ok(v) => v,
                Err(e) => panic!(e)
            };

            let walker = match &options.feature {
                Some(v) => glob(v).expect("feature glob is invalid"),
                None => match Path::new($featurepath).canonicalize() {
                    Ok(p) => {
                        GlobWalkerBuilder::new(p, "*.feature")
                            .case_insensitive(true)
                            .build()
                            .expect("feature path is invalid")
                    }
                    Err(e) => {
                        eprintln!("{}", e);
                        eprintln!("There was an error parsing \"{}\"; aborting.", $featurepath);
                        process::exit(1);
                    }
                }
            }.into_iter();

            let mut feature_files = walker
                .filter_map(Result::ok)
                .map(|entry| entry.path().to_owned())
                .collect::<Vec<_>>();
            feature_files.sort();

            let tests = Steps::combine($vec.iter().map(|f| f()));

            let mut output = DefaultOutput::default();

            let setup_fn: Option<fn() -> ()> = $setupfn;
            let before_fns: Option<&[fn(&Scenario) -> ()]> = $beforefns;
            let after_fns: Option<&[fn(&Scenario) -> ()]> = $afterfns;

            match setup_fn {
                Some(f) => f(),
                None => {}
            };

            if !tests.run(feature_files, before_fns, after_fns, options, &mut output) {
                process::exit(1);
            }
        }
    }
}

#[macro_export]
macro_rules! skip {
    () => {
        unimplemented!("cucumber test skipped");
    };
}

#[macro_export]
macro_rules! steps {
    (
        @step_type given
    ) => {
        $crate::StepType::Given
    };

    (
        @step_type when
    ) => {
        $crate::StepType::When
    };

    (
        @step_type then
    ) => {
        $crate::StepType::Then
    };

    (
        @parse_matches $worldtype:path, ($($arg_type:ty),*) $body:expr
    ) => {
        |world: &mut $worldtype, matches, step| {
            let body: fn(&mut $worldtype, $($arg_type,)* &$crate::Step) -> () = $body;
            let mut matches = matches.into_iter().enumerate().skip(1);

            body(
                world,
                $({
                    let (index, match_) = matches.next().unwrap();
                    match_.parse::<$arg_type>().unwrap_or_else(|_| panic!("Failed to parse {}th argument '{}' to type {}", index, match_, stringify!($arg_type)))
                },)*
                step
            )
        }
    };

    (
        @gather_steps, $worldtype:path, $tests:tt,
        $ty:ident regex $name:tt $body:expr;
    ) => {
        $tests.add_regex(steps!(@step_type $ty), $name, $body);
    };

    (
        @gather_steps, $worldtype:path, $tests:tt,
        $ty:ident regex $name:tt $body:expr; $( $items:tt )*
    ) => {
        $tests.add_regex(steps!(@step_type $ty), $name, $body);

        steps!(@gather_steps, $worldtype, $tests, $( $items )*);
    };

    (
        @gather_steps, $worldtype:path, $tests:tt,
        $ty:ident regex $name:tt ($($arg_type:ty),*) $body:expr;
    ) => {
        $tests.add_regex(steps!(@step_type $ty), $name, steps!(@parse_matches $worldtype, ($($arg_type),*) $body));
    };

    (
        @gather_steps, $worldtype:path, $tests:tt,
        $ty:ident regex $name:tt ($($arg_type:ty),*) $body:expr; $( $items:tt )*
    ) => {
        $tests.add_regex(steps!(@step_type $ty), $name, steps!(@parse_matches $worldtype, ($($arg_type),*) $body));

        steps!(@gather_steps, $worldtype, $tests, $( $items )*);
    };

    (
        @gather_steps, $worldtype:path, $tests:tt,
        $ty:ident $name:tt $body:expr;
    ) => {
        $tests.add_normal(steps!(@step_type $ty), $name, $body);
    };

    (
        @gather_steps, $worldtype:path, $tests:tt,
        $ty:ident $name:tt $body:expr; $( $items:tt )*
    ) => {
        $tests.add_normal(steps!(@step_type $ty), $name, $body);

        steps!(@gather_steps, $worldtype, $tests, $( $items )*);
    };

    (
        $worldtype:path => { $( $items:tt )* }
    ) => {
        pub fn steps() -> $crate::Steps<$worldtype> {
            let mut tests: $crate::Steps<$worldtype> = Default::default();
            steps!(@gather_steps, $worldtype, tests, $( $items )*);
            tests
        }
    };
}
