use std::env;
use std::fs;
use std::process::Command;

/// A helper function to compile a C string and execute the resulting binary.
fn run_e2e_test(test_name: &str, c_code: &str, expected_exit_code: i32) {
    // Set up a temporary directory for our generated files
    let out_dir = env::temp_dir().join("compiler_e2e_tests");
    fs::create_dir_all(&out_dir).expect("Failed to create test directory");

    let c_file = out_dir.join(format!("{}.c", test_name));
    let exe_file = out_dir.join(format!("{}.out", test_name));

    // Write the C code to disk
    fs::write(&c_file, c_code).expect("Failed to write C file");

    // Invoke compiler
    let compiler_exe = env!("CARGO_BIN_EXE_c-moon");

    let compile_status = Command::new(compiler_exe)
        .arg(c_file.to_str().unwrap())
        .arg("-o")
        .arg(exe_file.to_str().unwrap())
        .status()
        .expect("Failed to execute compiler process");

    assert!(
        compile_status.success(),
        "Compilation failed for test: {}",
        test_name
    );

    // Execute the generated x86 binary
    let run_status = Command::new(&exe_file)
        .status()
        .expect("Failed to execute the generated binary");

    // Assert the exit code
    assert_eq!(
        run_status.code(),
        Some(expected_exit_code),
        "Exit code mismatch for test: {}",
        test_name
    );
}

/// A helper function to compile(with optimizations) a C string and execute the resulting binary.
fn run_e2e_test_with_opt(test_name: &str, c_code: &str, expected_exit_code: i32) {
    // Set up a temporary directory for our generated files
    let out_dir = env::temp_dir().join("compiler_e2e_tests");
    fs::create_dir_all(&out_dir).expect("Failed to create test directory");

    let c_file = out_dir.join(format!("{}.c", test_name));
    let exe_file = out_dir.join(format!("{}.out", test_name));

    // Write the C code to disk
    fs::write(&c_file, c_code).expect("Failed to write C file");

    // Invoke compiler
    let compiler_exe = env!("CARGO_BIN_EXE_c-moon");

    let compile_status = Command::new(compiler_exe)
        .arg(c_file.to_str().unwrap())
        .arg("-o")
        .arg(exe_file.to_str().unwrap())
        .arg("--opt")
        .status()
        .expect("Failed to execute compiler process");

    assert!(
        compile_status.success(),
        "Compilation failed for test: {}",
        test_name
    );

    // Execute the generated x86 binary
    let run_status = Command::new(&exe_file)
        .status()
        .expect("Failed to execute the generated binary");

    // Assert the exit code
    assert_eq!(
        run_status.code(),
        Some(expected_exit_code),
        "Exit code mismatch for test: {}",
        test_name
    );
}

#[test]
fn test_return_42() {
    let code = "
        int main() {
            int a = 20;
            int b = 22;
            return a + b;
        }
    ";
    run_e2e_test("return_42", code, 42);
}

#[test]
fn test_return_42_with_opt() {
    let code = "
        int main() {
            int a = 20;
            int b = 22;
            return a + b;
        }
    ";
    run_e2e_test_with_opt("return_42_with_opt", code, 42);
}

#[test]
fn test_subtraction() {
    let code = "
        int main() {
            int a = 10;
            int b = 3;
            return a - b;
        }
    ";
    run_e2e_test("subtraction", code, 7);
}

#[test]
fn test_subtraction_with_opt() {
    let code = "
        int main() {
            int a = 10;
            int b = 3;
            return a - b;
        }
    ";
    run_e2e_test_with_opt("subtraction_with_opt", code, 7);
}

#[test]
fn test_multiplication() {
    let code = "
        int main() {
            int a = 10;
            int b = 3;
            return a * b;
        }
    ";
    run_e2e_test("test_multiplication", code, 30);
}

#[test]
fn test_multiplication_with_opt() {
    let code = "
        int main() {
            int a = 10;
            int b = 3;
            return a * b;
        }
    ";
    run_e2e_test_with_opt("test_multiplication_with_opt", code, 30);
}

#[test]
fn test_if_else() {
    let code = "
        int main() {
            int a = 1;
            if (a < 10) {
              return a;
            } else {
              return 2;
            }

            return 3;
        }
    ";
    run_e2e_test("test_if_else", code, 1);
}

#[test]
fn test_if_else_with_opt() {
    let code = "
        int main() {
            int a = 1;
            if (a < 10) {
              return a;
            } else {
              return 2;
            }

            return 3;
        }
    ";
    run_e2e_test_with_opt("test_if_else_with_opt", code, 1);
}

#[test]
fn test_while() {
    let code = "
        int main() {
            int i = 0;
            while (i < 10) {
              i = i + 1;
            }

            return i;
        }
    ";
    run_e2e_test("test_while", code, 10);
}

#[test]
fn test_while_with_opt() {
    let code = "
        int main() {
            int i = 0;
            while (i < 10) {
              i = i + 1;
            }

            return i;
        }
    ";
    run_e2e_test_with_opt("test_while_with_opt", code, 10);
}

#[test]
fn test_gt() {
    let code = "
        int main() {
            int a = 2;
            int b = 3;
            return a < b;
        }
    ";
    run_e2e_test("test_gt", code, 1);
}

#[test]
fn test_gt_with_opt() {
    let code = "
        int main() {
            int a = 2;
            int b = 3;
            return a < b;
        }
    ";
    run_e2e_test_with_opt("test_gt_with_opt", code, 1);
}

#[test]
fn test_gte() {
    let code = "
        int main() {
            int a = 2;
            int b = 2;
            return a <= b;
        }
    ";
    run_e2e_test("test_gte", code, 1);
}

#[test]
fn test_gte_with_opt() {
    let code = "
        int main() {
            int a = 2;
            int b = 2;
            return a <= b;
        }
    ";
    run_e2e_test_with_opt("test_gte_with_opt", code, 1);
}

#[test]
fn test_lt() {
    let code = "
        int main() {
            int a = 3;
            int b = 2;
            return a > b;
        }
    ";
    run_e2e_test("test_lt", code, 1);
}

#[test]
fn test_lt_with_opt() {
    let code = "
        int main() {
            int a = 3;
            int b = 2;
            return a > b;
        }
    ";
    run_e2e_test_with_opt("test_lt_with_opt", code, 1);
}

#[test]
fn test_lte() {
    let code = "
        int main() {
            int a = 2;
            int b = 2;
            return a >= b;
        }
    ";
    run_e2e_test("test_lte", code, 1);
}

#[test]
fn test_lte_with_opt() {
    let code = "
        int main() {
            int a = 2;
            int b = 2;
            return a >= b;
        }
    ";
    run_e2e_test_with_opt("test_lte_with_opt", code, 1);
}

#[test]
fn test_equal() {
    let code = "
        int main() {
            int a = 2;
            int b = 2;
            return a == b;
        }
    ";
    run_e2e_test("test_equal", code, 1);
}

#[test]
fn test_equal_with_opt() {
    let code = "
        int main() {
            int a = 2;
            int b = 2;
            return a == b;
        }
    ";
    run_e2e_test_with_opt("test_equal_with_opt", code, 1);
}

#[test]
fn test_not_equal() {
    let code = "
        int main() {
            int a = 2;
            int b = 3;
            return a != b;
        }
    ";
    run_e2e_test("test_not_equal", code, 1);
}

#[test]
fn test_not_equal_with_opt() {
    let code = "
        int main() {
            int a = 2;
            int b = 3;
            return a != b;
        }
    ";
    run_e2e_test_with_opt("test_not_equal_with_opt", code, 1);
}

#[test]
fn test_complex_expression() {
    let code = "
        int main() {
            int a = 1;
            int b = 2;
            int c = 2;
            return c * b + 10 - a * 2;
        }
    ";
    run_e2e_test("test_complex_expression", code, 12);
}

#[test]
fn test_complex_expression_with_opt() {
    let code = "
        int main() {
            int a = 1;
            int b = 2;
            int c = 2;
            return c * b + 10 - a * 2;
        }
    ";
    run_e2e_test_with_opt("test_not_equal_with_opt", code, 12);
}

#[test]
fn test_array_basic() {
    let code = "
        int main() {
            int arr[3];
            int i = 1;
            arr[0] = 10;
            arr[i] = 20;
            int x = arr[0] + arr[1];
            return x;
        }
    ";
    run_e2e_test("test_array_basic", code, 30);
}

#[test]
fn test_array_basic_with_opt() {
    let code = "
        int main() {
            int arr[3];
            int i = 1;
            arr[0] = 10;
            arr[i] = 20;
            int x = arr[0] + arr[1];
            return x;
        }
    ";
    run_e2e_test_with_opt("test_array_basic_with_opt", code, 30);
}

#[test]
fn test_array_loop_sum() {
    let code = "
        int main() {
            int arr[5];
            int i = 0;
            while (i < 5) {
                arr[i] = i + 1;
                i = i + 1;
            }
            int sum = arr[0] + arr[1] + arr[2] + arr[3] + arr[4];
            return sum;
        }
    ";
    // sum = 1 + 2 + 3 + 4 + 5 = 15
    run_e2e_test("test_array_loop_sum", code, 15);
}

#[test]
fn test_array_loop_sum_with_opt() {
    let code = "
        int main() {
            int arr[5];
            int i = 0;
            while (i < 5) {
                arr[i] = i + 1;
                i = i + 1;
            }
            int sum = arr[0] + arr[1] + arr[2] + arr[3] + arr[4];
            return sum;
        }
    ";
    // sum = 1 + 2 + 3 + 4 + 5 = 15
    run_e2e_test_with_opt("test_array_loop_sum_with_opt", code, 15);
}

#[test]
fn test_function_call() {
    let code = "
        int add(int a, int b) {
          return a + b;
        }

        int main() {
          int a = 1;
          int b = 1;

          return add(a, b);
        }
    ";
    run_e2e_test("test_function_call", code, 2);
}

#[test]
fn test_diagraphs() {
    let code = "
        int main() {
        int arr<:3:>;
        int i = 0;
        while (i < 3) <%
            arr<:i:> = i;
            i = i + 1;
        %>

        return arr<:2:>;
        }
    ";

    run_e2e_test("test_diagraphs", code, 2);
}

#[test]
fn test_function_call_with_opt() {
    let code = "
        int add(int a, int b) {
          return a + b;
        }

        int main() {
          int a = 1;
          int b = 1;

          return add(a, b);
        }
    ";
    run_e2e_test_with_opt("test_function_call_with_opt", code, 2);
}

#[test]
fn test_marco_object() {
    let code = "
        #define x 5
        int main() {
          int a = x;
          int b = x;

          return a + b + x;
        }
    ";
    run_e2e_test("test_macro_object", code, 15);
}

#[test]
fn test_marco_object_with_opt() {
    let code = "
        #define x 5
        int main() {
          int a = x;
          int b = x;

          return a + b + x;
        }
    ";
    run_e2e_test_with_opt("test_macro_object_with_opt", code, 15);
}

#[test]
fn test_marco_function() {
    let code = "
        #define ADD(a, b) (a + b)
        int main() {
          int a = 1;
          int b = 1;

          return ADD(a, b);
        }
    ";
    run_e2e_test("test_macro_function", code, 2);
}

#[test]
fn test_marco_function_with_opt() {
    let code = "
        #define ADD(a, b) (a + b)
        int main() {
          int a = 1;
          int b = 1;

          return ADD(a, b);
        }
    ";
    run_e2e_test_with_opt("test_macro_function_with_opt", code, 2);
}

// === Pointer Tests ===

#[test]
fn test_pointer_basic() {
    let code = "
        int main() {
            int x = 42;
            int *p = &x;
            return *p;
        }
    ";
    run_e2e_test("test_pointer_basic", code, 42);
}

#[test]
fn test_pointer_write_through() {
    let code = "
        int main() {
            int x = 10;
            int *p = &x;
            *p = 55;
            return x;
        }
    ";
    run_e2e_test("test_pointer_write_through", code, 55);
}

#[test]
fn test_pointer_function_param() {
    let code = "
        int deref(int *p) {
            return *p;
        }
        int main() {
            int x = 99;
            return deref(&x);
        }
    ";
    run_e2e_test("test_pointer_function_param", code, 99);
}

#[test]
fn test_pointer_to_pointer() {
    let code = "
        int main() {
            int x = 7;
            int *p = &x;
            int **pp = &p;
            return **pp;
        }
    ";
    run_e2e_test("test_pointer_to_pointer", code, 7);
}

#[test]
fn test_pointer_multiple_deref() {
    let code = "
        int main() {
            int a = 30;
            int b = 12;
            int *p = &a;
            int *q = &b;
            return *p + *q;
        }
    ";
    run_e2e_test("test_pointer_multiple_deref", code, 42);
}

#[test]
fn test_pointer_write_via_function() {
    let code = "
        void set_val(int *p, int v) {
            *p = v;
        }
        int main() {
            int x = 0;
            set_val(&x, 33);
            return x;
        }
    ";
    run_e2e_test("test_pointer_write_via_function", code, 33);
}

#[test]
fn test_pointer_basic_with_opt() {
    let code = "
        int main() {
            int x = 42;
            int *p = &x;
            return *p;
        }
    ";
    run_e2e_test_with_opt("test_pointer_basic_with_opt", code, 42);
}

#[test]
fn test_pointer_write_through_with_opt() {
    let code = "
        int main() {
            int x = 10;
            int *p = &x;
            *p = 55;
            return x;
        }
    ";
    run_e2e_test_with_opt("test_pointer_write_through_with_opt", code, 55);
}

#[test]
fn test_pointer_write_via_function_with_opt() {
    let code = "
        void set_val(int *p, int v) {
            *p = v;
        }
        int main() {
            int x = 0;
            set_val(&x, 33);
            return x;
        }
    ";
    run_e2e_test_with_opt("test_pointer_write_via_function_with_opt", code, 33);
}

#[test]
fn test_function_call_with_more_than_six_int_arguments() {
    let code = "
        int sum(int a, int b, int c, int d, int e, int f, int g, int h) {
            return a + b + c + d + e + f + g + h;
        }

        int main() {
            return sum(1, 2, 3, 4, 5, 6, 7, 8);
        }
    ";
    run_e2e_test(
        "test_function_call_with_more_than_six_int_arguments",
        code,
        36,
    );
}

#[test]
fn test_function_call_with_more_than_six_int_arguments_with_opt() {
    let code = "
        int sum(int a, int b, int c, int d, int e, int f, int g, int h) {
            return a + b + c + d + e + f + g + h;
        }

        int main() {
            return sum(1, 2, 3, 4, 5, 6, 7, 8);
        }
    ";
    run_e2e_test_with_opt(
        "test_function_call_with_more_than_six_int_arguments_with_opt",
        code,
        36,
    );
}

#[test]
fn test_function_call_with_seven_int_arguments() {
    // An odd number of stack arguments needs 8 bytes of alignment padding,
    // which must sit *below* them so argument 7 stays at [rbp + 16].
    let code = "
        int weighted(int a, int b, int c, int d, int e, int f, int g) {
            return a * 1 + b * 2 + c * 3 + d * 4 + e * 5 + f * 6 + g * 7;
        }

        int main() {
            return weighted(1, 1, 1, 1, 1, 1, 1);
        }
    ";
    run_e2e_test("test_function_call_with_seven_int_arguments", code, 28);
}

#[test]
fn test_function_call_with_seven_int_arguments_with_opt() {
    let code = "
        int weighted(int a, int b, int c, int d, int e, int f, int g) {
            return a * 1 + b * 2 + c * 3 + d * 4 + e * 5 + f * 6 + g * 7;
        }

        int main() {
            return weighted(1, 1, 1, 1, 1, 1, 1);
        }
    ";
    run_e2e_test_with_opt(
        "test_function_call_with_seven_int_arguments_with_opt",
        code,
        28,
    );
}

#[test]
fn test_function_call_with_more_than_six_arguments_via_pointer() {
    // Mixes an address-taken parameter (pinned to a stack slot) with
    // register- and stack-passed arguments.
    let code = "
        int bump(int *p, int b, int c, int d, int e, int f, int g, int h) {
            *p = *p + 1;
            return *p + b + c + d + e + f + g + h;
        }

        int main() {
            int v = 100;
            return bump(&v, 1, 2, 3, 4, 5, 6, 7) - 100;
        }
    ";
    run_e2e_test(
        "test_function_call_with_more_than_six_arguments_via_pointer",
        code,
        29,
    );
}

#[test]
fn test_value_live_across_a_call() {
    // `a` must survive the second call; a caller-saved register would be
    // destroyed by the callee.
    let code = "
        int f(int x) { int y = x + 1; int z = y + 1; return z * 2; }

        int main() {
            int a = f(1);
            int b = f(2);
            return a + b;
        }
    ";
    run_e2e_test("test_value_live_across_a_call", code, 14);
}

#[test]
fn test_value_live_across_a_call_with_opt() {
    let code = "
        int f(int x) { int y = x + 1; int z = y + 1; return z * 2; }

        int main() {
            int a = f(1);
            int b = f(2);
            return a + b;
        }
    ";
    run_e2e_test_with_opt("test_value_live_across_a_call_with_opt", code, 14);
}

#[test]
fn test_more_values_live_across_a_call_than_callee_saved_registers() {
    // Eight values are live across the call but only five callee-saved
    // registers exist, so the rest have to reach the stack.
    let code = "
        int f(int x) { return x * 2; }

        int main() {
            int a = 1; int b = 2; int c = 3; int d = 4;
            int e = 5; int g = 6; int h = 7; int i = 8;
            int r = f(3);
            return a + b + c + d + e + g + h + i + r;
        }
    ";
    run_e2e_test(
        "test_more_values_live_across_a_call_than_callee_saved_registers",
        code,
        42,
    );
}

#[test]
fn test_recursive_call_keeps_intermediate_result() {
    // The result of `fib(n - 1)` is live across the `fib(n - 2)` call.
    let code = "
        int fib(int n) {
            if (n < 2) { return n; }
            return fib(n - 1) + fib(n - 2);
        }

        int main() { return fib(10); }
    ";
    run_e2e_test("test_recursive_call_keeps_intermediate_result", code, 55);
}

#[test]
fn test_recursive_call_keeps_intermediate_result_with_opt() {
    let code = "
        int fib(int n) {
            if (n < 2) { return n; }
            return fib(n - 1) + fib(n - 2);
        }

        int main() { return fib(10); }
    ";
    run_e2e_test_with_opt(
        "test_recursive_call_keeps_intermediate_result_with_opt",
        code,
        55,
    );
}

#[test]
fn test_call_inside_loop_keeps_loop_state() {
    // The accumulator and the counter are both live across the call.
    let code = "
        int add(int a, int b) { return a + b; }

        int main() {
            int s = 0;
            int i = 0;
            while (i < 10) {
                s = add(s, i);
                i = i + 1;
            }
            return s;
        }
    ";
    run_e2e_test("test_call_inside_loop_keeps_loop_state", code, 45);
}

#[test]
fn test_call_inside_loop_keeps_loop_state_with_opt() {
    let code = "
        int add(int a, int b) { return a + b; }

        int main() {
            int s = 0;
            int i = 0;
            while (i < 10) {
                s = add(s, i);
                i = i + 1;
            }
            return s;
        }
    ";
    run_e2e_test_with_opt("test_call_inside_loop_keeps_loop_state_with_opt", code, 45);
}

#[test]
fn test_nested_calls_with_more_than_six_arguments() {
    // Two eight-argument calls whose results are combined: the first
    // result is live across the second call.
    let code = "
        int inner(int a, int b, int c, int d, int e, int f, int g, int h) {
            return a - b + c - d + e - f + g - h;
        }

        int outer(int a, int b, int c, int d, int e, int f, int g, int h) {
            return inner(h, g, f, e, d, c, b, a) + inner(a, b, c, d, e, f, g, h);
        }

        int main() { return outer(9, 8, 7, 6, 5, 4, 3, 2) + 50; }
    ";
    run_e2e_test("test_nested_calls_with_more_than_six_arguments", code, 50);
}

#[test]
fn test_nested_calls_with_more_than_six_arguments_with_opt() {
    let code = "
        int inner(int a, int b, int c, int d, int e, int f, int g, int h) {
            return a - b + c - d + e - f + g - h;
        }

        int outer(int a, int b, int c, int d, int e, int f, int g, int h) {
            return inner(h, g, f, e, d, c, b, a) + inner(a, b, c, d, e, f, g, h);
        }

        int main() { return outer(9, 8, 7, 6, 5, 4, 3, 2) + 50; }
    ";
    run_e2e_test_with_opt(
        "test_nested_calls_with_more_than_six_arguments_with_opt",
        code,
        50,
    );
}

#[test]
fn test_unused_parameter_does_not_clobber_a_later_one() {
    // `p3` is never read, so its live interval collapses and the allocator
    // may hand its register to `p4`.  Only the live parameter's copy may
    // survive, or `p4` is destroyed before it is ever used.
    let code = "
        int pick(int p0, int p1, int p2, int p3, int p4, int p5, int p6) {
            int v0 = p6 - p0;
            if (p2 < p5) { p0 = p5 - 1; } else { p1 = p0 + 4; }
            return p4 * 1 + p1 + v0 * 0;
        }

        int main() { return pick(4, 3, 1, 5, 4, 2, 2); }
    ";
    run_e2e_test(
        "test_unused_parameter_does_not_clobber_a_later_one",
        code,
        7,
    );
}

#[test]
fn test_unused_parameter_does_not_clobber_a_later_one_with_opt() {
    let code = "
        int pick(int p0, int p1, int p2, int p3, int p4, int p5, int p6) {
            int v0 = p6 - p0;
            if (p2 < p5) { p0 = p5 - 1; } else { p1 = p0 + 4; }
            return p4 * 1 + p1 + v0 * 0;
        }

        int main() { return pick(4, 3, 1, 5, 4, 2, 2); }
    ";
    run_e2e_test_with_opt(
        "test_unused_parameter_does_not_clobber_a_later_one_with_opt",
        code,
        7,
    );
}
