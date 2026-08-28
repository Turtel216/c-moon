//@ compile-flags: --opt

// A count past the width of the type is undefined in C, so the fold is free to
// answer with whatever the machine would have: an `int` shift reads five bits
// of its count, which makes a shift by 33 a shift by one.

// CHECK-LABEL: main:
// CHECK-NOT: shl
// CHECK: mov rax, 2

int main() {
    int one = 1;
    return one << 33;
}
