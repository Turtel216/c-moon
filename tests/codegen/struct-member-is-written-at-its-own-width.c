// A member occupies exactly as many bytes as its type, so a write to one
// touches that many and no more -- the bytes next to it belong to the member
// after it, or to the padding that keeps it aligned.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK: mov BYTE PTR
// CHECK: mov DWORD PTR
// CHECK-NOT: mov QWORD PTR

struct Pair {
    char tag;
    int n;
};

int main() {
    struct Pair p;
    p.tag = 1;
    p.n = 7;
    return p.n + p.tag;
}
