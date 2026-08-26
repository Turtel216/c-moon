// A member the struct does not have, offered the one it was probably meant to be.

struct Point {
    int counter;
    int y;
};

int main() {
    struct Point p;
    return p.countr;
}
