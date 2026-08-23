// An element address survives being passed to a function, which writes the
// element through it.
//@ exit-code: 9

int set(int *slot, int value) {
    *slot = value;
    return value;
}

int main() {
    int arr[3];
    int i = 1;
    arr[0] = 0;
    arr[1] = 0;
    arr[2] = 0;

    set(&arr[i], 9);

    return arr[1];
}
