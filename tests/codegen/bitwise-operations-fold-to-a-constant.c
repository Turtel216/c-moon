//@ compile-flags: --opt

// Nothing about a mask over known bits has to wait until run time.

// CHECK-LABEL: main:
// CHECK-NOT: and
// CHECK-NOT: xor
// CHECK: mov rax, 40

int main() {
    int a = 60;
    int b = 13;
    return ((a & b) | (a ^ b)) & 42;
}
