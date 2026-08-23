// The address of an element points at the element itself, so a write through
// it is visible when the array is read by index.
//@ exit-code: 42

int main() {
    int arr[3];
    int i = 2;
    arr[0] = 1;
    arr[1] = 2;
    arr[2] = 3;

    int *first = &arr[0];
    int *last = &arr[i];
    *last = 41;

    return *first + arr[2];
}
