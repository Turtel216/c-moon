// A whole expression tree folds to a single literal: 2 * 2 + 10 - 1.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK-NOT: imul
// CHECK-NOT: add
// CHECK: mov rax, 13

int main() {
    int a = 1;
    int b = 2;
    int c = 2;
    return c * b + 10 - a;
}
