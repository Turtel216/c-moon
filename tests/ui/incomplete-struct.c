// A tag that was declared but never defined has no size, so no object of it
// can exist -- though a pointer to one still can.

struct Point;

int main() {
    struct Point p;
    return 0;
}
