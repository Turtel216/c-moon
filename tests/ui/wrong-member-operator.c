// `.` reaches into a struct value; a pointer to one needs `->`.

struct Point {
    int x;
};

int main() {
    struct Point p;
    struct Point *q = &p;
    return q.x;
}
