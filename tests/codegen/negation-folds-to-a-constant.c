// A negated constant is folded like any other subtraction, leaving no
// arithmetic behind.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK-NOT: sub
// CHECK: mov rax, -5

int main() {
    int five = 5;
    return -five;
}
