// A `void` function returning through its side effect on a pointer.
//@ exit-code: 33

void set_val(int *p, int v) {
    *p = v;
}

int main() {
    int x = 0;
    set_val(&x, 33);
    return x;
}
