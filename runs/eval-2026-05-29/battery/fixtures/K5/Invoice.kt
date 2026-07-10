package shop

// The billing document sent to a customer.
data class Invoice(
    val number: String,
    val amountDue: Double,
    val paid: Boolean = false,
)
