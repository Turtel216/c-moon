// An `int` in memory occupies four bytes, so a write through a pointer to one
// touches a dword and no more -- the neighbouring bytes belong to something
// else.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK: mov DWORD PTR
// CHECK-NOT: mov QWORD PTR [r

int main() {
    int x = 0;
    int *p = &x;
    *p = 7;
    return x;
}
