// A function declared but never defined here compiles to nothing of its own:
// the call names the symbol and the linker supplies the address, so neither a
// body nor a `.globl` is emitted for it.

// CHECK-NOT: .globl abs
// CHECK-LABEL: main:
// CHECK: call abs
// CHECK-NOT: .globl abs

extern int abs(int x);

int main() {
    return abs(-1);
}
