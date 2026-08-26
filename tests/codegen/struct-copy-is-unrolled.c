// A struct assignment copies a known number of bytes, so it becomes a straight
// run of the widest moves that fit: no loop, and no call to a library routine.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK-NOT: call
// CHECK: mov QWORD PTR
// CHECK-NOT: call

struct Pair {
    long int a;
    long int b;
};

int main() {
    struct Pair x;
    x.a = 3;
    x.b = 4;

    struct Pair y = x;
    return (int)(y.a + y.b);
}
