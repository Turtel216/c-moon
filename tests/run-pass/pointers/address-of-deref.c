// `&*p` is `p`: the two operators cancel, so taking the address of a
// dereference yields the pointer back rather than the address of the loaded
// value, which is a temporary and has none.
//@ exit-code: 42

int main() {
    int x = 42;
    int *p = &x;
    int *q = &*p;
    return *q;
}
