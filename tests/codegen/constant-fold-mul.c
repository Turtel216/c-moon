// A constant multiplication folds, so no `imul` is emitted.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK-NOT: imul
// CHECK: mov rax, 1000

int main() {
    int a = 10;
    int b = 100;
    return a * b;
}
