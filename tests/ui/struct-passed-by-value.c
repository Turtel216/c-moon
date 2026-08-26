// Splitting a struct across argument registers is the System V classification,
// which this compiler does not implement: a struct travels through a pointer.

struct Point {
    int x;
};

int take(struct Point p) {
    return p.x;
}

int main() {
    return 0;
}
