// A struct assignment copies the whole object, so writing the copy afterwards
// leaves the original alone.
//@ exit-code: 48

struct Point {
    int x;
    int y;
};

int main() {
    struct Point a;
    a.x = 1;
    a.y = 2;

    // Copy-initialisation and plain assignment take the same path.
    struct Point b = a;
    struct Point c;
    c = b;

    c.x = 40;

    return a.x + a.y + b.x + b.y + c.x + c.y;
}
