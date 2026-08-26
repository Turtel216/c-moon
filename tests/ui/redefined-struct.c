// A struct tag may be defined only once.

struct Point {
    int x;
};

struct Point {
    int y;
};

int main() {
    return 0;
}
