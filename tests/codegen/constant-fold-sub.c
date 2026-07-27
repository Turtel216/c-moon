// A constant subtraction folds to its result.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK-NOT: sub rax
// CHECK: mov rax, 2

int main() {
    int a = 22;
    int b = 20;
    return a - b;
}
