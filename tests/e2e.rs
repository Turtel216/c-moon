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
