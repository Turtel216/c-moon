// Constant propagation and folding collapse the addition to a literal, so no
// arithmetic instruction survives into the emitted code.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK-NOT: add
// CHECK: mov rax, 42

int main() {
    int a = 20;
    int b = 22;
    return a + b;
}
