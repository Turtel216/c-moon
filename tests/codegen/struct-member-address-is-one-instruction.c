// A member of a local struct sits at a fixed distance into the frame storage
// the object was given, so its address is a single `lea` with a constant
// displacement rather than an addition applied to the object's own address.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK-NOT: add
// CHECK: lea
// CHECK-NEXT: mov DWORD PTR

struct Point {
    int x;
    int y;
};

int main() {
    struct Point p;
    p.y = 7;
    return p.y;
}
