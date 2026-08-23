// `&*p` is `p`, so the pair emits no code at all: the only address computed
// in this function is the one `&x` asks for.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK: lea
// CHECK-NOT: lea

int main() {
    int x = 5;
    int *p = &x;
    int *q = &*p;
    return *q;
}
