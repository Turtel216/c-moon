// A `char` in memory occupies one byte, so a write through a pointer to one
// touches a byte and no more -- the neighbouring bytes belong to something
// else.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK: mov BYTE PTR
// CHECK-NOT: mov DWORD PTR [r
// CHECK-NOT: mov QWORD PTR [r

int main() {
    char c = 0;
    char *p = &c;
    *p = 7;
    return c;
}
