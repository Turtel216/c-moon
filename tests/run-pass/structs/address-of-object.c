// The address of a struct and of one of its members. An aggregate already has
// storage, so its address is where that storage is rather than a slot pinned
// for it.
//@ exit-code: 33

struct Point {
    int x;
    int y;
};

int main() {
    struct Point p;
    p.x = 1;
    p.y = 2;

    struct Point *whole = &p;
    int *member = &p.y;

    *member = 30;
    whole->x = whole->x + 2;

    return p.x + p.y;
}
