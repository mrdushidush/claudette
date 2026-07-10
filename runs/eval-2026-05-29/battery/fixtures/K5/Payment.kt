package shop

// Decoy: an object and a plain class, no Invoice here.
object PaymentGateway {
    fun charge(amountDue: Double): Boolean = amountDue > 0.0
}

class PaymentError(message: String) : Exception(message)
