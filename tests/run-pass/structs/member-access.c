// Reading and writing a member of a local struct.
//@ exit-code: 42

struct Point {
    int x;
    int y;
};

int main() {
    struct Point p;
    p.x = 10;
    p.y = 32;
    return p.x + p.y;
}
